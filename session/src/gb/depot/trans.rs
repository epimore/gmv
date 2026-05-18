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
                        (entity.cb)(Err(GlobalError::new_biz_error(1000, "response timeout", |msg| {
                            error!("{msg}")
                        })));
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
