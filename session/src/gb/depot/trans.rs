use crate::gb::depot::Callback;
use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::io::send_sip_pkt_out;
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::net::state::{Association, Zip};
use base::tokio;
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc::Sender;
use base::tokio::sync::Notify;
use base::tokio::time;
use base::tokio::time::Instant;
use base::tokio_util::sync::CancellationToken;
use parking_lot::RwLock;
use rsip::headers::UntypedHeader;
use rsip::{Headers, Method, Request, Response};
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use std::ops::Add;
use std::sync::Arc;
use std::time::Duration;

const EXPIRE_TTL: Duration = Duration::from_secs(2);

// 事务状态机
pub enum CurrentState {
    Proceeding, // 处理中 (收到1xx)
    Completed,  // 完成 (收到2xx-6xx)
    Terminated, // 终止
}

/// 事务 key
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
struct TransEntity {
    current_state: Option<CurrentState>,
    retry_count: u8, //重试2次
    msg: Bytes,
    association: Association,
    expire_instant: Instant,
    cb: Callback,
}

impl TransactionIdentifier for rsip::SipMessage {}

struct State {
    anti_map: HashMap<String, TransEntity>,
    expire_set: BTreeSet<(Instant, String)>,
}
impl State {
    fn next_expiration(&self) -> Option<Instant> {
        self.expire_set.first().map(|expiration| expiration.0)
    }
}
struct Shared {
    state: RwLock<State>,
    background_task: Notify,
    output: Sender<Zip>,
}
impl Shared {
    fn next_step(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut guard = self.state.write();
        let state = &mut *guard;
        while let Some((when, key)) = state.expire_set.iter().next() {
            if when > &now {
                return Some(*when);
            }

            match state.anti_map.entry(key.to_string()) {
                Entry::Occupied(mut occ) => {
                    let nk = occ.key().to_string();
                    let entity = occ.get_mut();
                    state
                        .expire_set
                        .remove(&(entity.expire_instant, key.to_string()));
                    if entity.retry_count < 2 {
                        let expire = now + EXPIRE_TTL;
                        entity.retry_count += 1;
                        entity.expire_instant = expire;
                        send_sip_pkt_out(
                            &self.output,
                            entity.msg.clone(),
                            entity.association.clone(),
                            Some("Trans"),
                        );
                        state.expire_set.insert((expire, nk));
                    } else {
                        let entity1 = occ.remove();
                        (entity1.cb)(Err(GlobalError::new_biz_error(
                            1000,
                            "响应超时",
                            |msg| error!("{msg}"),
                        )));
                    }
                }
                Entry::Vacant(_) => {}
            }
        }
        None
    }
    async fn expire_retry_clean(shared: Arc<Shared>, cancel_token: CancellationToken) {
        loop {
            if cancel_token.is_cancelled() {
                break;
            }
            if let Some(when) = shared.next_step() {
                tokio::select! {
                    _ = time::sleep_until(when) =>{},
                    _ = shared.background_task.notified() =>{},
                }
            } else {
                shared.background_task.notified().await;
            }
        }
        let mut guard = shared.state.write();
        guard.expire_set.clear();
        guard.anti_map.clear();
    }
}
pub struct TransactionContext {
    shared: Arc<Shared>,
}
impl TransactionContext {
    pub fn init(rt: Handle, cancel_token: CancellationToken, output: Sender<Zip>) -> Self {
        let ctx = Self {
            shared: Arc::new(Shared {
                state: RwLock::new(State {
                    anti_map: Default::default(),
                    expire_set: Default::default(),
                }),
                background_task: Notify::new(),
                output,
            }),
        };
        let shared = ctx.shared.clone();
        rt.spawn(Shared::expire_retry_clean(shared, cancel_token));
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
        let expire_instant = Instant::now().add(EXPIRE_TTL);
        let entity = TransEntity {
            current_state: None,
            retry_count: 0,
            msg: Bytes::from(request),
            association,
            expire_instant,
            cb,
        };
        let mut state = self.shared.state.write();
        let state = &mut *state;
        let should_notify = state.next_expiration().map(|ts| ts > expire_instant).unwrap_or(true);
        match state.anti_map.entry(key) {
            Entry::Occupied(occ) => Err(GlobalError::new_sys_error(
                "事务中已存在该请求，请稍后尝试",
                |msg| error!("{}:{msg}", occ.key()),
            ))?,
            Entry::Vacant(vac) => {
                state.expire_set.insert((expire_instant, vac.key().clone()));
                vac.insert(entity);
            }
        }
        if should_notify {
            self.shared.background_task.notify_one();
        }
        Ok(())
    }
    pub fn handle_response(&self, response: Response) -> GlobalResult<()> {
        let key = response.generate_trans_key()?;
        let mut guard = self.shared.state.write();
        let state = &mut *guard;
        match state.anti_map.entry(key) {
            Entry::Occupied(mut occ) => {
                let entity = occ.get_mut();
                match &response.status_code.code() {
                    100..=199 => {
                        entity.current_state = Some(CurrentState::Proceeding);
                        let expire = Instant::now().add(EXPIRE_TTL);
                        let old_instant = entity.expire_instant;
                        entity.expire_instant = expire;
                        state
                            .expire_set
                            .remove(&(old_instant, occ.key().to_string()));
                        state.expire_set.insert((expire, occ.key().to_string()));
                    }
                    _ => {
                        let (key, entity) = occ.remove_entry();
                        (entity.cb)(Ok(response));
                        state.expire_set.remove(&(entity.expire_instant, key));
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
