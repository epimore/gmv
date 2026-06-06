use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use parking_lot::RwLock;
use rsip::headers::UntypedHeader;
use rsip::{Headers, Method, Request, Response};

use crate::gb::depot::Callback;
use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::io::send_sip_pkt_out;
use crate::register::core::Register;

use base::bytes::Bytes;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::net::state::{Association, Zip};
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc::Sender;
use base::tokio_util::sync::CancellationToken;

const EXPIRE_TTL: Duration = Duration::from_secs(2);
static TRANS_CTX: OnceLock<Arc<TransactionContext>> = OnceLock::new();

pub enum CurrentState {
    Proceeding,
    Completed,
    Terminated,
}

pub trait TransactionIdentifier: Send + Sync + HeaderItemExt {
    fn generate_trans_key(&self) -> GlobalResult<String> {
        let key = if self.method_by_cseq()? == Method::Invite {
            format!("INVITE:{}", self.branch()?.value())
        } else {
            format!(
                "{}:{}:{}",
                self.cs_eq()?.value(),
                self.branch()?.value(),
                self.call_id()?.value()
            )
        };
        Ok(key)
    }
}

impl TransactionIdentifier for rsip::Request {}
impl TransactionIdentifier for rsip::Response {}
impl TransactionIdentifier for rsip::SipMessage {}

struct TransEntity {
    current_state: Option<CurrentState>,
    retry_count: u8,
    msg: Bytes,
    association: Association,
    cb: Callback,
}

struct State {
    anti_map: HashMap<String, TransEntity>,
}

struct Shared {
    state: RwLock<State>,
    output: Sender<Zip>,
}

impl Shared {
    fn fail_association(&self, association: &Association) {
        let entities = {
            let mut state = self.state.write();
            let keys = state
                .anti_map
                .iter()
                .filter_map(|(key, entity)| {
                    (entity.association == *association).then(|| key.clone())
                })
                .collect::<Vec<_>>();
            keys.into_iter()
                .filter_map(|key| state.anti_map.remove(&key))
                .collect::<Vec<_>>()
        };

        for entity in entities {
            (entity.cb)(Err(GlobalError::new_biz_error(
                BaseErrorCode::Network.code(),
                "sip tcp connection closed",
                |msg| error!("{msg}: association={association}"),
            )));
        }
    }

    fn next_step(&self, keys: Vec<String>) {
        let mut guard = self.state.write();
        let state = &mut *guard;
        for key in keys {
            match state.anti_map.entry(key.clone()) {
                Entry::Occupied(mut occ) => {
                    let entity = occ.get_mut();
                    if entity.retry_count < 2 {
                        entity.retry_count += 1;
                        send_sip_pkt_out(
                            &self.output,
                            entity.msg.clone(),
                            entity.association.clone(),
                            Some("Trans"),
                        );
                        let _ = Register::scheduler().insert_transaction(key, EXPIRE_TTL);
                    } else {
                        let entity = occ.remove();
                        (entity.cb)(Err(GlobalError::new_biz_error(
                            BaseErrorCode::Timeout.code(),
                            "response timeout",
                            |msg| error!("{msg}"),
                        )));
                    }
                }
                Entry::Vacant(_) => {}
            }
        }
    }
}

pub struct TransactionContext {
    shared: Arc<Shared>,
}

impl TransactionContext {
    pub fn init(_rt: Handle, _cancel_token: CancellationToken, output: Sender<Zip>) -> Arc<Self> {
        let ctx = Arc::new(Self {
            shared: Arc::new(Shared {
                state: RwLock::new(State {
                    anti_map: Default::default(),
                }),
                output,
            }),
        });
        let _ = TRANS_CTX.set(ctx.clone());
        ctx
    }

    fn global() -> &'static Arc<Self> {
        TRANS_CTX.get().expect("TransactionContext not initialized")
    }

    pub fn handle_timeout_keys(keys: Vec<String>) {
        Self::global().shared.next_step(keys);
    }

    pub fn handle_connection_closed(association: &Association) {
        if let Some(ctx) = TRANS_CTX.get() {
            ctx.shared.fail_association(association);
        }
    }

    pub fn process_request(
        &self,
        request: Request,
        association: Association,
        cb: Callback,
    ) -> GlobalResult<()> {
        if Self::no_response(&request) {
            let response = rsip::Response {
                status_code: 200.into(),
                headers: Headers::default(),
                version: rsip::Version::V2,
                body: Default::default(),
            };
            cb(Ok(response));
            return Ok(());
        }
        let key = (&request).generate_trans_key()?;
        let entity = TransEntity {
            current_state: None,
            retry_count: 0,
            msg: Bytes::from(request),
            association,
            cb,
        };
        let mut state = self.shared.state.write();
        match state.anti_map.entry(key.clone()) {
            Entry::Occupied(occ) => Err(GlobalError::new_sys_error(
                "transaction already exists for request",
                |msg| error!("{}:{msg}", occ.key()),
            ))?,
            Entry::Vacant(vac) => {
                vac.insert(entity);
                let _ = Register::scheduler().insert_transaction(key, EXPIRE_TTL);
            }
        }
        Ok(())
    }

    pub fn handle_response(&self, response: Response) -> GlobalResult<()> {
        let key = response.generate_trans_key()?;
        let mut guard = self.shared.state.write();
        let state = &mut *guard;
        match state.anti_map.entry(key.clone()) {
            Entry::Occupied(mut occ) => {
                let entity = occ.get_mut();
                match response.status_code.code() {
                    100..=199 => {
                        entity.current_state = Some(CurrentState::Proceeding);
                        let _ = Register::scheduler().refresh_transaction(&key);
                    }
                    _ => {
                        let (_key, entity) = occ.remove_entry();
                        let _ = Register::scheduler().remove_transaction(&key);
                        (entity.cb)(Ok(response));
                    }
                }
            }
            Entry::Vacant(vac) => Err(GlobalError::new_sys_error(
                "unknown or expired response dropped",
                |msg| warn!("{}:{msg}", vac.key()),
            ))?,
        }
        Ok(())
    }

    fn no_response(request: &Request) -> bool {
        matches!(request.method(), Method::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::{Shared, State, TransEntity};
    use base::bytes::Bytes;
    use base::net::state::{Association, Protocol};
    use base::tokio::sync::{mpsc, oneshot};
    use parking_lot::RwLock;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn association(port: u16) -> Association {
        Association::new(
            "0.0.0.0:25600".parse().unwrap(),
            format!("127.0.0.1:{port}").parse().unwrap(),
            Protocol::TCP,
        )
    }

    #[test]
    fn connection_close_fails_only_matching_transactions() {
        let (output, _rx) = mpsc::channel(1);
        let shared = Shared {
            state: RwLock::new(State {
                anti_map: HashMap::new(),
            }),
            output,
        };
        let closed = association(40001);
        let other = association(40002);
        let (closed_tx, closed_rx) = oneshot::channel();
        let (other_tx, mut other_rx) = oneshot::channel();

        {
            let mut state = shared.state.write();
            state.anti_map.insert(
                "closed".to_string(),
                TransEntity {
                    current_state: None,
                    retry_count: 0,
                    msg: Bytes::new(),
                    association: closed.clone(),
                    cb: Box::new(move |result| {
                        let _ = closed_tx.send(result);
                    }),
                },
            );
            state.anti_map.insert(
                "other".to_string(),
                TransEntity {
                    current_state: None,
                    retry_count: 0,
                    msg: Bytes::new(),
                    association: other,
                    cb: Box::new(move |result| {
                        let _ = other_tx.send(result);
                    }),
                },
            );
        }

        shared.fail_association(&closed);

        assert!(closed_rx.blocking_recv().unwrap().is_err());
        assert!(other_rx.try_recv().is_err());
        assert!(shared.state.read().anti_map.contains_key("other"));
    }
}
