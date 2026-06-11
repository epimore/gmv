//! LEGACY RSIP PIPELINE
//!
//! The medium-term SIP stack has moved to `crate::gb::sip` + `gmv_pjsip`.
//! This file is kept temporarily for compatibility with existing service APIs
//! and for migration reference. New code must not add SIP parsing, transaction,
//! dialog, CSeq/tag/branch, or header-generation logic here.
//!

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use rsip::headers::UntypedHeader;
use rsip::message::HeadersExt;
use rsip::{Header, Headers, Method, Request, Response};

use crate::gb::depot::Callback;
use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::io::send_sip_pkt_out;
use crate::register::core::Register;

use base::bytes::Bytes;
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::net::state::{Association, Protocol, Zip};
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc::Sender;
use base::tokio_util::sync::CancellationToken;

const SIP_T1: Duration = Duration::from_millis(500);
const SIP_T2: Duration = Duration::from_secs(4);
const SIP_TRANSACTION_TIMEOUT: Duration = Duration::from_secs(32);
static TRANS_CTX: OnceLock<Arc<TransactionContext>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClientTransactionState {
    Calling,
    Proceeding,
}

struct TransactionTimer {
    method: Method,
    reliable: bool,
    state: ClientTransactionState,
    deadline: Instant,
    retransmit_interval: Duration,
}

impl TransactionTimer {
    fn new(method: Method, protocol: Protocol, now: Instant) -> Self {
        Self {
            method,
            reliable: matches!(protocol, Protocol::TCP),
            state: ClientTransactionState::Calling,
            deadline: now + SIP_TRANSACTION_TIMEOUT,
            retransmit_interval: SIP_T1,
        }
    }

    fn on_provisional(&mut self) {
        self.state = ClientTransactionState::Proceeding;
    }

    fn on_retransmit(&mut self) {
        let next = self.retransmit_interval.saturating_mul(2);
        self.retransmit_interval = if self.method == Method::Invite {
            next
        } else {
            next.min(SIP_T2)
        };
    }

    fn should_retransmit(&self) -> bool {
        if self.reliable {
            return false;
        }
        self.method != Method::Invite || self.state == ClientTransactionState::Calling
    }

    fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    fn next_delay(&self, now: Instant) -> Duration {
        let remaining = self.deadline.saturating_duration_since(now);
        if self.should_retransmit() {
            self.retransmit_interval.min(remaining)
        } else {
            remaining
        }
    }
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
    timer: TransactionTimer,
    request: Request,
    msg: Bytes,
    association: Association,
    cb: Callback,
}

#[derive(Clone)]
struct CachedAck {
    expires_at: Instant,
    msg: Bytes,
    association: Association,
}

enum ResponseAction {
    Provisional,
    Complete(TransEntity),
    ResendAck(CachedAck),
}

struct State {
    anti_map: HashMap<String, TransEntity>,
    completed_acks: HashMap<String, CachedAck>,
    dialog_acks: HashMap<String, CachedAck>,
}

fn dialog_ack_key<T: HeaderItemExt>(message: &T) -> GlobalResult<String> {
    Ok(format!(
        "{}:{}",
        message.call_id()?.value(),
        message.seq()?
    ))
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
            let entities = keys
                .into_iter()
                .filter_map(|key| {
                    if let Some(scheduler) = crate::register::schedule::TimeScheduler::try_global() {
                        let _ = scheduler.remove_transaction(&key);
                    }
                    state.anti_map.remove(&key)
                })
                .collect::<Vec<_>>();
            state
                .completed_acks
                .retain(|_, ack| ack.association != *association);
            state
                .dialog_acks
                .retain(|_, ack| ack.association != *association);
            entities
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
        let mut callbacks = Vec::new();
        for key in keys {
            let mut state = self.state.write();
            match state.anti_map.entry(key.clone()) {
                Entry::Occupied(mut occ) => {
                    let entity = occ.get_mut();
                    let now = Instant::now();
                    if entity.timer.expired(now) {
                        let entity = occ.remove();
                        callbacks.push(entity.cb);
                    } else {
                        if entity.timer.should_retransmit() {
                            send_sip_pkt_out(
                                &self.output,
                                entity.msg.clone(),
                                entity.association.clone(),
                                Some("Trans"),
                            );
                            entity.timer.on_retransmit();
                        }
                        let delay = entity.timer.next_delay(now);
                        let _ = Register::scheduler().insert_transaction(key, delay);
                    }
                }
                Entry::Vacant(_) => {}
            }
        }
        for cb in callbacks {
            cb(Err(GlobalError::new_biz_error(
                BaseErrorCode::Timeout.code(),
                "response timeout",
                |msg| error!("{msg}"),
            )));
        }
    }
}

fn build_non_2xx_ack(request: &Request, response: &Response) -> GlobalResult<Request> {
    let mut headers = request.headers.clone();
    headers.retain(|header| {
        matches!(
            header,
            Header::Via(_)
                | Header::From(_)
                | Header::CallId(_)
                | Header::Route(_)
                | Header::MaxForwards(_)
                | Header::Authorization(_)
                | Header::ProxyAuthorization(_)
                | Header::UserAgent(_)
        )
    });
    headers.push(
        response
            .to_header()
            .map_err(|err| {
                GlobalError::new_sys_error("non-2xx INVITE response missing To", |msg| {
                    error!("{msg}: {err}")
                })
            })?
            .clone()
            .into(),
    );
    let seq = request.cseq_header().map_err(|err| {
        GlobalError::new_sys_error("INVITE request missing CSeq", |msg| error!("{msg}: {err}"))
    })?;
    let seq = seq.seq().map_err(|err| {
        GlobalError::new_sys_error("invalid INVITE CSeq", |msg| error!("{msg}: {err}"))
    })?;
    headers.push(rsip::headers::CSeq::new(format!("{seq} ACK")).into());
    headers.push(rsip::headers::ContentLength::default().into());
    Ok(Request {
        method: Method::Ack,
        uri: request.uri.clone(),
        headers,
        version: request.version.clone(),
        body: Default::default(),
    })
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
                    completed_acks: Default::default(),
                    dialog_acks: Default::default(),
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
            if request.method == Method::Ack {
                if let Ok(key) = dialog_ack_key(&request) {
                    self.shared.state.write().dialog_acks.insert(
                        key,
                        CachedAck {
                            expires_at: Instant::now() + SIP_TRANSACTION_TIMEOUT,
                            msg: Bytes::from(request.clone()),
                            association: association.clone(),
                        },
                    );
                }
            }
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
        let now = Instant::now();
        let timer = TransactionTimer::new(
            request.method.clone(),
            association.protocol,
            now,
        );
        let delay = timer.next_delay(now);
        let entity = TransEntity {
            timer,
            msg: Bytes::from(request.clone()),
            request,
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
                let _ = Register::scheduler().insert_transaction(key, delay);
            }
        }
        Ok(())
    }

    pub fn handle_response(&self, response: Response) -> GlobalResult<()> {
        let key = response.generate_trans_key()?;
        let status = response.status_code.code();
        let action = {
            let mut state = self.shared.state.write();
            let now = Instant::now();
            state
                .completed_acks
                .retain(|_, ack| ack.expires_at > now);
            state.dialog_acks.retain(|_, ack| ack.expires_at > now);
            if (100..=199).contains(&status) {
                let Some(entity) = state.anti_map.get_mut(&key) else {
                    return Err(GlobalError::new_sys_error(
                        "unknown or expired provisional response dropped",
                        |msg| warn!("{key}:{msg}"),
                    ));
                };
                entity.timer.on_provisional();
                let delay = entity.timer.next_delay(now);
                let _ = Register::scheduler().remove_transaction(&key);
                let _ = Register::scheduler().insert_transaction(key.clone(), delay);
                ResponseAction::Provisional
            } else if let Some(entity) = state.anti_map.remove(&key) {
                let _ = Register::scheduler().remove_transaction(&key);
                ResponseAction::Complete(entity)
            } else if let Some(ack) = state.completed_acks.get(&key).cloned() {
                ResponseAction::ResendAck(ack)
            } else if (200..300).contains(&status)
                && response.method_by_cseq()? == Method::Invite
            {
                let ack_key = dialog_ack_key(&response)?;
                let Some(ack) = state.dialog_acks.get(&ack_key).cloned() else {
                    return Err(GlobalError::new_sys_error(
                        "repeated INVITE 2xx has no dialog ACK",
                        |msg| warn!("{key}:{msg}"),
                    ));
                };
                ResponseAction::ResendAck(ack)
            } else {
                return Err(GlobalError::new_sys_error(
                    "unknown or expired response dropped",
                    |msg| warn!("{key}:{msg}"),
                ));
            }
        };
        match action {
            ResponseAction::Provisional => {}
            ResponseAction::ResendAck(ack) => {
                send_sip_pkt_out(
                    &self.shared.output,
                    ack.msg,
                    ack.association,
                    Some("INVITE repeated final ACK"),
                );
            }
            ResponseAction::Complete(entity) => {
                if entity.request.method == Method::Invite
                    && (300..=699).contains(&status)
                {
                    let association = entity.association.clone();
                    let ack = build_non_2xx_ack(&entity.request, &response)?;
                    let msg = Bytes::from(ack);
                    send_sip_pkt_out(
                        &self.shared.output,
                        msg.clone(),
                        association.clone(),
                        Some("INVITE non-2xx ACK"),
                    );
                    self.shared.state.write().completed_acks.insert(
                        key,
                        CachedAck {
                            expires_at: Instant::now() + SIP_TRANSACTION_TIMEOUT,
                            msg,
                            association,
                        },
                    );
                }
                (entity.cb)(Ok(response));
            }
        }
        Ok(())
    }

    fn no_response(request: &Request) -> bool {
        matches!(request.method(), Method::Ack)
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ClientTransactionState, Shared, State, TransEntity, TransactionContext,
        TransactionTimer, build_non_2xx_ack,
    };
    use base::bytes::Bytes;
    use base::net::state::{Association, Protocol};
    use base::tokio::sync::{mpsc, oneshot};
    use parking_lot::RwLock;
    use rsip::message::HeadersExt;
    use rsip::prelude::UntypedHeader;
    use rsip::{Header, Method, Request, Response, StatusCode, Uri, Version};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

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
                completed_acks: HashMap::new(),
                dialog_acks: HashMap::new(),
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
                    timer: TransactionTimer::new(Method::Message, Protocol::TCP, Instant::now()),
                    request: Request {
                        method: Method::Message,
                        uri: Uri::try_from("sip:device@example.com").unwrap(),
                        headers: Default::default(),
                        version: Version::V2,
                        body: Default::default(),
                    },
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
                    timer: TransactionTimer::new(Method::Message, Protocol::TCP, Instant::now()),
                    request: Request {
                        method: Method::Message,
                        uri: Uri::try_from("sip:device@example.com").unwrap(),
                        headers: Default::default(),
                        version: Version::V2,
                        body: Default::default(),
                    },
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

    #[test]
    fn reliable_transport_waits_for_final_timeout_without_retransmission() {
        let now = Instant::now();
        let timer = TransactionTimer::new(Method::Invite, Protocol::TCP, now);

        assert_eq!(timer.state, ClientTransactionState::Calling);
        assert_eq!(timer.next_delay(now), Duration::from_secs(32));
        assert!(!timer.should_retransmit());
    }

    #[test]
    fn udp_invite_uses_timer_a_and_stops_retransmitting_after_provisional_response() {
        let now = Instant::now();
        let mut timer = TransactionTimer::new(Method::Invite, Protocol::UDP, now);

        assert_eq!(timer.next_delay(now), Duration::from_millis(500));
        assert!(timer.should_retransmit());

        timer.on_retransmit();
        assert_eq!(timer.next_delay(now), Duration::from_secs(1));

        timer.on_provisional();
        assert_eq!(timer.state, ClientTransactionState::Proceeding);
        assert!(!timer.should_retransmit());
        assert_eq!(timer.next_delay(now), Duration::from_secs(32));
    }

    #[test]
    fn udp_non_invite_retransmit_interval_is_capped_at_t2() {
        let now = Instant::now();
        let mut timer = TransactionTimer::new(Method::Message, Protocol::UDP, now);

        for _ in 0..8 {
            timer.on_retransmit();
        }

        assert_eq!(timer.next_delay(now), Duration::from_secs(4));
        assert!(timer.should_retransmit());
    }

    #[test]
    fn non_2xx_invite_ack_reuses_original_transaction_identity() {
        let request = Request {
            method: Method::Invite,
            uri: Uri::try_from("sip:device@192.0.2.10:5060").unwrap(),
            headers: vec![
                rsip::headers::Via::new(
                    "SIP/2.0/UDP platform.example.com;branch=z9hG4bK-original",
                )
                .into(),
                rsip::headers::From::new(
                    "<sip:platform@example.com>;tag=platform-tag",
                )
                .into(),
                rsip::headers::To::new("<sip:device@example.com>").into(),
                rsip::headers::CallId::new("call-id").into(),
                rsip::headers::CSeq::new("42 INVITE").into(),
                rsip::headers::MaxForwards::new("70").into(),
                rsip::headers::Contact::new("<sip:platform@example.com>").into(),
                rsip::headers::Subject::new("channel:1,platform:0").into(),
                rsip::headers::ContentType::new("application/sdp").into(),
                rsip::headers::ContentLength::new("4").into(),
            ]
            .into(),
            version: Version::V2,
            body: b"test".to_vec(),
        };
        let response = Response {
            status_code: StatusCode::BusyHere,
            headers: vec![
                rsip::headers::Via::new(
                    "SIP/2.0/UDP platform.example.com;branch=z9hG4bK-original",
                )
                .into(),
                rsip::headers::From::new(
                    "<sip:platform@example.com>;tag=platform-tag",
                )
                .into(),
                rsip::headers::To::new(
                    "<sip:device@example.com>;tag=device-tag",
                )
                .into(),
                rsip::headers::CallId::new("call-id").into(),
                rsip::headers::CSeq::new("42 INVITE").into(),
                rsip::headers::ContentLength::default().into(),
            ]
            .into(),
            version: Version::V2,
            body: Default::default(),
        };

        let ack = build_non_2xx_ack(&request, &response).unwrap();

        assert_eq!(ack.method, Method::Ack);
        assert_eq!(ack.uri, request.uri);
        assert_eq!(ack.cseq_header().unwrap().value(), "42 ACK");
        assert_eq!(
            ack.to_header().unwrap().value(),
            "<sip:device@example.com>;tag=device-tag"
        );
        assert!(ack.headers.iter().any(
            |header| matches!(header, Header::Via(via) if via.value().contains("z9hG4bK-original"))
        ));
        assert!(ack.body.is_empty());
        assert!(!ack.headers.iter().any(
            |header| matches!(header, Header::Contact(_) | Header::Subject(_))
        ));
        assert!(ack.headers.iter().any(
            |header| matches!(header, Header::ContentLength(value) if value.value() == "0")
        ));
    }

    #[test]
    fn repeated_invite_2xx_reuses_cached_dialog_ack() {
        let (output, mut output_rx) = mpsc::channel(1);
        let context = TransactionContext {
            shared: Arc::new(Shared {
                state: RwLock::new(State {
                    anti_map: HashMap::new(),
                    completed_acks: HashMap::new(),
                    dialog_acks: HashMap::new(),
                }),
                output,
            }),
        };
        let association = Association::new(
            "0.0.0.0:25600".parse().unwrap(),
            "127.0.0.1:40003".parse().unwrap(),
            Protocol::UDP,
        );
        let ack = Request {
            method: Method::Ack,
            uri: Uri::try_from("sip:device@example.com").unwrap(),
            headers: vec![
                rsip::headers::Via::new(
                    "SIP/2.0/UDP platform.example.com;branch=z9hG4bK-ack",
                )
                .into(),
                rsip::headers::From::new(
                    "<sip:platform@example.com>;tag=platform-tag",
                )
                .into(),
                rsip::headers::To::new(
                    "<sip:device@example.com>;tag=device-tag",
                )
                .into(),
                rsip::headers::CallId::new("dialog-call-id").into(),
                rsip::headers::CSeq::new("42 ACK").into(),
                rsip::headers::ContentLength::default().into(),
            ]
            .into(),
            version: Version::V2,
            body: Default::default(),
        };
        context
            .process_request(ack, association, Box::new(|_| {}))
            .unwrap();
        let repeated = Response {
            status_code: StatusCode::OK,
            headers: vec![
                rsip::headers::Via::new(
                    "SIP/2.0/UDP platform.example.com;branch=z9hG4bK-invite",
                )
                .into(),
                rsip::headers::From::new(
                    "<sip:platform@example.com>;tag=platform-tag",
                )
                .into(),
                rsip::headers::To::new(
                    "<sip:device@example.com>;tag=device-tag",
                )
                .into(),
                rsip::headers::CallId::new("dialog-call-id").into(),
                rsip::headers::CSeq::new("42 INVITE").into(),
                rsip::headers::ContentLength::default().into(),
            ]
            .into(),
            version: Version::V2,
            body: Default::default(),
        };

        context.handle_response(repeated).unwrap();

        assert!(output_rx.try_recv().is_ok());
    }
}
