use std::net::Ipv4Addr;
use std::sync::{Arc, mpsc as std_mpsc};
use std::thread;
use std::time::Duration;

use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc;
use base::tokio::task::JoinHandle;
use base::tokio::time;
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::{
    AuthAlgorithm, AuthCredential, CredentialKind, SipAuthLookupResult, SipOutboundMessage,
    SipRuntime, SipRuntimeConfig, SipRuntimeEvent, SipRuntimeEventKind,
};

use super::adapter::{GbSipEvent, apply_business_event};
use super::auth::{AUTH_DB_BATCH_LIMIT, DeviceAuthCache};
use super::message::GbMessageEvent;
use super::register::GbRegisterEvent;

const AUTH_BATCH_WINDOW: Duration = Duration::from_millis(5);
const AUTH_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PENDING_AUTH: u32 = 20_000;

struct AuthLookup {
    lookup_id: u64,
    device_id: String,
    realm: String,
}

struct AuthCompletion {
    lookup_id: u64,
    result: SipAuthLookupResult,
}

enum RuntimeCommand {
    CompleteAuth(AuthCompletion),
    SendMessage(SipOutboundMessage),
}

pub struct NativeSipRuntimeService {
    cancel: CancellationToken,
    runtime_commands: std_mpsc::Sender<RuntimeCommand>,
    auth_task: JoinHandle<()>,
    event_task: JoinHandle<()>,
    runtime_thread: Option<thread::JoinHandle<()>>,
    udp_port: Option<u16>,
    tcp_port: Option<u16>,
}

impl NativeSipRuntimeService {
    pub fn start(
        bind_address: Ipv4Addr,
        port: u16,
        realm: String,
        auth_cache: Arc<DeviceAuthCache>,
        cancel: CancellationToken,
    ) -> GlobalResult<(Self, mpsc::UnboundedReceiver<SipRuntimeEvent>)> {
        let (lookup_tx, lookup_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (business_tx, business_rx) = mpsc::unbounded_channel();
        let (runtime_command_tx, runtime_command_rx) = std_mpsc::channel();
        let (startup_tx, startup_rx) = std_mpsc::sync_channel(1);

        let service_cancel = cancel.child_token();
        let auth_cancel = service_cancel.child_token();
        let auth_task = Handle::current().spawn(run_auth_batches(
            lookup_rx,
            runtime_command_tx.clone(),
            auth_cache,
            auth_cancel,
        ));
        let event_task = Handle::current().spawn(run_native_business_events(
            business_rx,
            service_cancel.child_token(),
        ));

        let runtime_cancel = service_cancel.child_token();
        let runtime_thread = thread::Builder::new()
            .name("gmv-pjsip-owner".into())
            .spawn(move || {
                let config = SipRuntimeConfig {
                    bind_address,
                    port,
                    auth_realm: realm,
                    auth_lookup_timeout: AUTH_LOOKUP_TIMEOUT,
                    max_pending_auth: MAX_PENDING_AUTH,
                    ..SipRuntimeConfig::default()
                };
                let (mut runtime, events) = match SipRuntime::start(config) {
                    Ok(started) => started,
                    Err(err) => {
                        let _ = startup_tx.send(Err(err.to_string()));
                        return;
                    }
                };
                let ports = (runtime.udp_port(), runtime.tcp_port());
                if startup_tx.send(Ok(ports)).is_err() {
                    return;
                }

                while !runtime_cancel.is_cancelled() {
                    while let Ok(command) = runtime_command_rx.try_recv() {
                        match command {
                            RuntimeCommand::CompleteAuth(completion) => {
                                if let Err(err) = runtime
                                    .complete_auth_lookup(completion.lookup_id, completion.result)
                                {
                                    warn!(
                                        "complete native SIP auth lookup failed: \
                                         lookup_id={}, err={err}",
                                        completion.lookup_id
                                    );
                                }
                            }
                            RuntimeCommand::SendMessage(message) => {
                                if let Err(err) = runtime.send_message(&message) {
                                    warn!(
                                        "send native SIP MESSAGE failed: operation_id={}, \
                                         err={err}",
                                        message.operation_id
                                    );
                                }
                            }
                        }
                    }

                    match events.recv_timeout(Duration::from_millis(2)) {
                        Ok(event) => {
                            if event.kind == SipRuntimeEventKind::AuthLookupRequired {
                                let lookup = event
                                    .lookup_id
                                    .zip(event.device_id.clone())
                                    .zip(event.realm.clone())
                                    .map(|((lookup_id, device_id), realm)| AuthLookup {
                                        lookup_id,
                                        device_id,
                                        realm,
                                    });
                                if let Some(lookup) = lookup {
                                    if lookup_tx.send(lookup).is_err() {
                                        break;
                                    }
                                } else {
                                    warn!("native SIP auth event missing lookup identity");
                                }
                            } else {
                                let _ = business_tx.send(event.clone());
                                if event_tx.send(event).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(std_mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }

                if let Err(err) = runtime.stop() {
                    warn!("stop native SIP runtime failed: {err}");
                }
            })
            .map_err(|err| {
                GlobalError::new_sys_error(
                    &format!("spawn native SIP runtime thread failed: {err}"),
                    |msg| error!("{msg}"),
                )
            })?;

        let (udp_port, tcp_port) = startup_rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|err| {
                GlobalError::new_sys_error(
                    &format!("wait native SIP runtime startup failed: {err}"),
                    |msg| error!("{msg}"),
                )
            })?
            .map_err(|err| {
                GlobalError::new_sys_error(
                    &format!("start native SIP runtime failed: {err}"),
                    |msg| error!("{msg}"),
                )
            })?;

        Ok((
            Self {
                cancel: service_cancel,
                runtime_commands: runtime_command_tx,
                auth_task,
                event_task,
                runtime_thread: Some(runtime_thread),
                udp_port,
                tcp_port,
            },
            event_rx,
        ))
    }

    pub fn udp_port(&self) -> Option<u16> {
        self.udp_port
    }

    pub fn tcp_port(&self) -> Option<u16> {
        self.tcp_port
    }

    pub fn send_message(&self, message: SipOutboundMessage) -> GlobalResult<()> {
        self.runtime_commands
            .send(RuntimeCommand::SendMessage(message))
            .map_err(|err| {
                GlobalError::new_sys_error(
                    &format!("queue native SIP MESSAGE failed: {err}"),
                    |msg| error!("{msg}"),
                )
            })
    }

    pub fn shutdown(mut self) {
        self.cancel.cancel();
        self.auth_task.abort();
        self.event_task.abort();
        if let Some(thread) = self.runtime_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for NativeSipRuntimeService {
    fn drop(&mut self) {
        self.cancel.cancel();
        self.auth_task.abort();
        self.event_task.abort();
        if let Some(thread) = self.runtime_thread.take() {
            let _ = thread.join();
        }
    }
}

async fn run_native_business_events(
    mut events: mpsc::UnboundedReceiver<SipRuntimeEvent>,
    cancel: CancellationToken,
) {
    loop {
        let event = base::tokio::select! {
            event = events.recv() => event,
            _ = cancel.cancelled() => break,
        };
        let Some(event) = event else {
            break;
        };
        let business_event = GbRegisterEvent::from_native(&event)
            .map(GbSipEvent::Register)
            .or_else(|| GbMessageEvent::from_native(&event).map(GbSipEvent::Message));
        if let Some(business_event) = business_event {
            if let Err(err) = apply_business_event(&business_event) {
                warn!("apply native SIP business event failed: {err}");
            }
        }
    }
}

async fn run_auth_batches(
    mut lookups: mpsc::UnboundedReceiver<AuthLookup>,
    runtime_commands: std_mpsc::Sender<RuntimeCommand>,
    auth_cache: Arc<DeviceAuthCache>,
    cancel: CancellationToken,
) {
    loop {
        let first = base::tokio::select! {
            lookup = lookups.recv() => lookup,
            _ = cancel.cancelled() => break,
        };
        let Some(first) = first else {
            break;
        };

        let mut batch = Vec::with_capacity(AUTH_DB_BATCH_LIMIT);
        batch.push(first);
        time::sleep(AUTH_BATCH_WINDOW).await;
        while batch.len() < AUTH_DB_BATCH_LIMIT {
            match lookups.try_recv() {
                Ok(lookup) => batch.push(lookup),
                Err(_) => break,
            }
        }

        let keys = batch
            .iter()
            .map(|lookup| (lookup.device_id.clone(), lookup.realm.clone()))
            .collect::<Vec<_>>();
        match auth_cache.get_or_load_many(&keys).await {
            Ok(oauths) => {
                for (lookup, oauth) in batch.into_iter().zip(oauths) {
                    let result = auth_result(lookup.device_id, lookup.realm, oauth);
                    if runtime_commands
                        .send(RuntimeCommand::CompleteAuth(AuthCompletion {
                            lookup_id: lookup.lookup_id,
                            result,
                        }))
                        .is_err()
                    {
                        return;
                    }
                }
            }
            Err(err) => {
                error!("batch native SIP auth lookup failed: {err}");
                for lookup in batch {
                    if runtime_commands
                        .send(RuntimeCommand::CompleteAuth(AuthCompletion {
                            lookup_id: lookup.lookup_id,
                            result: SipAuthLookupResult::Reject,
                        }))
                        .is_err()
                    {
                        return;
                    }
                }
            }
        }
    }
}

fn auth_result(
    device_id: String,
    realm: String,
    oauth: Option<crate::storage::entity::GmvOauth>,
) -> SipAuthLookupResult {
    let Some(oauth) = oauth else {
        return SipAuthLookupResult::NotFound;
    };
    if oauth.status == 0 {
        return SipAuthLookupResult::Reject;
    }
    if oauth.pwd_check == 0 {
        return SipAuthLookupResult::Bypass;
    }
    let Some(secret) = oauth.pwd.filter(|password| !password.is_empty()) else {
        return SipAuthLookupResult::Reject;
    };
    SipAuthLookupResult::Credential(AuthCredential {
        username: device_id,
        realm,
        secret,
        kind: CredentialKind::PlainPassword,
        algorithm: AuthAlgorithm::Md5,
    })
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, UdpSocket};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    use base::tokio::runtime::Runtime;
    use base::tokio::time;
    use base::tokio_util::sync::CancellationToken;
    use gmv_pjsip::{SipAuthLookupResult, SipOutboundMessage, SipRuntimeEventKind};

    use super::{NativeSipRuntimeService, auth_result};
    use crate::gb::sip::auth::DeviceAuthCache;
    use crate::storage::entity::GmvOauth;

    static RUNTIME_TEST_LOCK: Mutex<()> = Mutex::new(());

    fn oauth(pwd_check: u8, pwd: Option<&str>) -> GmvOauth {
        GmvOauth {
            device_id: "34020000001110000009".into(),
            domain_id: "34020000002000000001".into(),
            domain: "3402000000".into(),
            pwd: pwd.map(ToOwned::to_owned),
            pwd_check,
            alias: None,
            status: 1,
            heartbeat_sec: 60,
        }
    }

    #[test]
    fn maps_cached_device_policy_to_native_auth_completion() {
        assert!(matches!(
            auth_result("device".into(), "realm".into(), Some(oauth(0, None))),
            SipAuthLookupResult::Bypass
        ));
        assert!(matches!(
            auth_result(
                "device".into(),
                "realm".into(),
                Some(oauth(1, Some("secret")))
            ),
            SipAuthLookupResult::Credential(_)
        ));
        assert!(matches!(
            auth_result("device".into(), "realm".into(), None),
            SipAuthLookupResult::NotFound
        ));
    }

    #[test]
    fn session_bridge_owns_native_runtime_on_dedicated_thread() {
        let _guard = RUNTIME_TEST_LOCK.lock().expect("lock native runtime tests");
        let runtime = Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let cancel = CancellationToken::new();
            let (service, _events) = NativeSipRuntimeService::start(
                Ipv4Addr::LOCALHOST,
                0,
                "3402000000".into(),
                Arc::new(DeviceAuthCache::default()),
                cancel.clone(),
            )
            .expect("start native SIP service");
            let port = service.udp_port().expect("native UDP port");
            let socket = UdpSocket::bind("127.0.0.1:0").expect("bind OPTIONS client");
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set OPTIONS timeout");
            let local = socket.local_addr().expect("OPTIONS local address");
            let request = format!(
                "OPTIONS sip:127.0.0.1:{port} SIP/2.0\r\n\
Via: SIP/2.0/UDP {local};branch=z9hG4bK-session-native;rport\r\n\
From: <sip:test@127.0.0.1>;tag=session-native\r\n\
To: <sip:gmv@127.0.0.1>\r\n\
Call-ID: session-native-loopback\r\n\
CSeq: 1 OPTIONS\r\n\
Max-Forwards: 70\r\n\
Content-Length: 0\r\n\r\n"
            );
            socket
                .send_to(request.as_bytes(), ("127.0.0.1", port))
                .expect("send OPTIONS");
            let mut response = [0u8; 2048];
            let (len, _) = socket
                .recv_from(&mut response)
                .expect("receive OPTIONS response");
            assert!(String::from_utf8_lossy(&response[..len]).starts_with("SIP/2.0 200"));
            service.shutdown();
            assert!(!cancel.is_cancelled());
        });
    }

    #[test]
    fn session_bridge_queues_outbound_message_on_owner_thread() {
        let _guard = RUNTIME_TEST_LOCK.lock().expect("lock native runtime tests");
        let runtime = Runtime::new().expect("create Tokio runtime");
        runtime.block_on(async {
            let peer = UdpSocket::bind("127.0.0.1:0").expect("bind outbound peer");
            peer.set_read_timeout(Some(Duration::from_secs(2)))
                .expect("set outbound peer timeout");
            let peer_port = peer.local_addr().expect("outbound peer address").port();
            let responder = thread::spawn(move || {
                let mut packet = [0u8; 4096];
                let (len, source) = peer.recv_from(&mut packet).expect("receive MESSAGE");
                let request = String::from_utf8_lossy(&packet[..len]);
                let header = |name: &str| {
                    request
                        .lines()
                        .find_map(|line| {
                            line.split_once(':')
                                .filter(|(key, _)| key.eq_ignore_ascii_case(name))
                                .map(|(_, value)| value.trim().to_owned())
                        })
                        .unwrap_or_else(|| panic!("missing {name}"))
                };
                let response = format!(
                    "SIP/2.0 200 OK\r\n\
Via: {}\r\n\
From: {}\r\n\
To: {};tag=session-outbound\r\n\
Call-ID: {}\r\n\
CSeq: {}\r\n\
Content-Length: 0\r\n\r\n",
                    header("Via"),
                    header("From"),
                    header("To"),
                    header("Call-ID"),
                    header("CSeq")
                );
                peer.send_to(response.as_bytes(), source)
                    .expect("send MESSAGE response");
            });

            let (service, mut events) = NativeSipRuntimeService::start(
                Ipv4Addr::LOCALHOST,
                0,
                "3402000000".into(),
                Arc::new(DeviceAuthCache::default()),
                CancellationToken::new(),
            )
            .expect("start native SIP service");
            let operation_id = 77;
            service
                .send_message(SipOutboundMessage {
                    operation_id,
                    target_uri: format!("sip:device@127.0.0.1:{peer_port}"),
                    from_uri: format!(
                        "<sip:platform@127.0.0.1:{}>",
                        service.udp_port().expect("native UDP port")
                    ),
                    content_type: "Application/MANSCDP+xml".into(),
                    body: b"<Query><CmdType>Catalog</CmdType></Query>".to_vec(),
                })
                .expect("queue native MESSAGE");
            let event = time::timeout(Duration::from_secs(2), events.recv())
                .await
                .expect("wait outbound response event")
                .expect("receive outbound response event");
            assert_eq!(event.kind, SipRuntimeEventKind::OutboundResponse);
            assert_eq!(event.operation_id, Some(operation_id));
            assert_eq!(event.status_code, Some(200));

            responder.join().expect("join outbound peer");
            service.shutdown();
        });
    }
}
