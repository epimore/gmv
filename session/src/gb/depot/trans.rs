use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::time::Duration;
use std::sync::Arc;

use parking_lot::RwLock;
use rsip::headers::UntypedHeader;
use rsip::{Headers, Method, Request, Response};

use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::depot::Callback;
use crate::gb::io::send_sip_pkt_out;
use crate::register::schedule::{ScheduleEvent, TimeScheduler};
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::net::state::{Association, Zip};
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc::Sender;
use base::tokio_util::sync::CancellationToken;

const EXPIRE_TTL: Duration = Duration::from_secs(2);

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
    async fn expire_retry_clean(
        shared: Arc<Shared>,
        time_schedule: TimeScheduler<String>,
        cancel_token: CancellationToken,
    ) {
        while let Some(batch) = time_schedule.next_batch(&cancel_token).await {
            shared.next_step(batch, &time_schedule);
        }
        let mut guard = shared.state.write();
        guard.anti_map.clear();
    }

    fn next_step(&self, batch: Vec<ScheduleEvent<String>>, time_schedule: &TimeScheduler<String>) {
        let mut guard = self.state.write();
        let state = &mut *guard;
        for event in batch {
            match state.anti_map.entry(event.key) {
                Entry::Occupied(mut occ) => {
                    let key = occ.key().to_string();
                    let entity = occ.get_mut();
                    if entity.retry_count < 2 {
                        entity.retry_count += 1;
                        send_sip_pkt_out(
                            &self.output,
                            entity.msg.clone(),
                            entity.association.clone(),
                            Some("Trans"),
                        );
                        let _ = time_schedule.insert(key, EXPIRE_TTL);
                    } else {
                        let entity = occ.remove();
                        (entity.cb)(Err(GlobalError::new_biz_error(
                            1000,
                            "响应超时",
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
    time_schedule: TimeScheduler<String>,
}

impl TransactionContext {
    pub fn init(rt: Handle, cancel_token: CancellationToken, output: Sender<Zip>) -> Self {
        let ctx = Self {
            shared: Arc::new(Shared {
                state: RwLock::new(State {
                    anti_map: Default::default(),
                }),
                output,
            }),
            time_schedule: TimeScheduler::new(),
        };
        let shared = ctx.shared.clone();
        let time_schedule = ctx.time_schedule.clone();
        rt.spawn(Shared::expire_retry_clean(shared, time_schedule, cancel_token));
        ctx
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
                "事务中已存在该请求，请稍后尝试",
                |msg| error!("{}:{msg}", occ.key()),
            ))?,
            Entry::Vacant(vac) => {
                vac.insert(entity);
                let _ = self.time_schedule.insert(key, EXPIRE_TTL);
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
                        let _ = self.time_schedule.insert(key, EXPIRE_TTL);
                    }
                    _ => {
                        let (_key, entity) = occ.remove_entry();
                        let _ = self.time_schedule.remove(&key);
                        (entity.cb)(Ok(response));
                    }
                }
            }
            Entry::Vacant(vac) => Err(GlobalError::new_sys_error(
                "未知或超时响应:丢弃",
                |msg| warn!("{}:{msg}", vac.key()),
            ))?,
        }
        Ok(())
    }

    fn no_response(request: &Request) -> bool {
        matches!(request.method(), Method::Ack)
    }

    fn dis_retry(request: &Request) -> bool {
        if let body = request.body() {
            if let Ok(body_str) = std::str::from_utf8(body) {
                return matches!(request.method(), Method::Message)
                    && body_str.contains("<CmdType>Keepalive</CmdType>");
            }
        }
        false
    }
}
