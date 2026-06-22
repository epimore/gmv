use std::net::{Ipv4Addr, SocketAddr};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, mpsc as std_mpsc};
use std::thread;
use std::time::Duration;

use base::cfg_lib::{CliBasic, default_cli_basic};
use base::dashmap::DashMap;
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, warn};
use base::net::state::{Association, Protocol};
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc;
use base::tokio::task::JoinHandle;
use base::tokio::time;
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::{
    AuthAlgorithm, AuthCredential, CredentialKind, SipAuthLookupResult, SipDialogRequest,
    SipInviteResponse, SipOutboundInvite, SipOutboundMessage, SipOutboundSubscribe,
    SipRestoredDialogRequest, SipRuntime, SipRuntimeConfig, SipRuntimeEvent, SipRuntimeEventKind,
    SipRuntimeSockets, SipTransportProtocol,
};

use super::adapter::{GbSipEvent, apply_business_event};
use super::auth::{AUTH_DB_BATCH_LIMIT, DeviceAuthCache};
use super::bye::GbByeEvent;
use super::invite::GbIncomingInviteEvent;
use super::message::GbMessageEvent;
use super::register::GbRegisterEvent;
use super::runtime_cache::SipRuntimeCache;
use crate::register::core::Register;

const AUTH_BATCH_WINDOW: Duration = Duration::from_millis(5);
const AUTH_LOOKUP_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_PENDING_AUTH: u32 = 20_000;
const RUNTIME_COMMAND_CAPACITY: usize = 32_768;
pub const NATIVE_SIP_IO_CAPACITY: usize = 32_768;
static NATIVE_SIP_RUNTIME: OnceLock<NativeSipRuntimeHandle> = OnceLock::new();
#[cfg(test)]
pub(crate) static RUNTIME_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn cli_basic() -> CliBasic {
    default_cli_basic!()
}

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
    SendInvite(SipOutboundInvite),
    SendDialog(SipDialogRequest),
    SendRestoredDialog(SipRestoredDialogRequest),
    RespondInvite(SipInviteResponse),
    SendSubscribe(SipOutboundSubscribe),
    CloseTransport { association_id: u64, status: i32 },
}

#[derive(Clone)]
pub struct NativeSipRuntimeHandle {
    runtime_commands: std_mpsc::SyncSender<RuntimeCommand>,
    association_ids: Arc<DashMap<Association, u64>>,
    next_operation_id: Arc<AtomicU64>,
}

impl NativeSipRuntimeHandle {
    pub fn install_global(&self) -> GlobalResult<()> {
        NATIVE_SIP_RUNTIME.set(self.clone()).map_err(|_| {
            GlobalError::new_sys_error("native SIP runtime is already initialized", |msg| {
                error!("{msg}")
            })
        })
    }

    pub fn global() -> GlobalResult<&'static Self> {
        NATIVE_SIP_RUNTIME.get().ok_or_else(|| {
            GlobalError::new_sys_error("native SIP runtime is not initialized", |msg| {
                error!("{msg}")
            })
        })
    }

    pub fn next_operation_id(&self) -> u64 {
        self.next_operation_id.fetch_add(1, Ordering::Relaxed)
    }

    pub fn send_message(
        &self,
        association: &Association,
        mut message: SipOutboundMessage,
    ) -> GlobalResult<()> {
        let protocol = native_protocol(association.protocol)?;
        message.protocol = protocol;
        message.association_id = if protocol == SipTransportProtocol::Tcp {
            self.association_ids
                .get(association)
                .map(|entry| *entry.value())
                .ok_or_else(|| {
                    GlobalError::new_sys_error(
                        "TCP association is not registered in native SIP runtime",
                        |msg| error!("{msg}: association={association:?}"),
                    )
                })?
        } else {
            0
        };
        self.try_send(RuntimeCommand::SendMessage(message))
    }

    pub fn send_invite(
        &self,
        association: &Association,
        mut invite: SipOutboundInvite,
    ) -> GlobalResult<()> {
        let protocol = native_protocol(association.protocol)?;
        invite.protocol = protocol;
        invite.association_id = if protocol == SipTransportProtocol::Tcp {
            self.association_ids
                .get(association)
                .map(|entry| *entry.value())
                .ok_or_else(|| {
                    GlobalError::new_sys_error(
                        "TCP association is not registered in native SIP runtime",
                        |msg| error!("{msg}: association={association:?}"),
                    )
                })?
        } else {
            0
        };
        self.try_send(RuntimeCommand::SendInvite(invite))
    }

    pub fn send_dialog_request(&self, request: SipDialogRequest) -> GlobalResult<()> {
        self.try_send(RuntimeCommand::SendDialog(request))
    }

    pub fn send_restored_dialog_request(
        &self,
        association: Option<&Association>,
        mut request: SipRestoredDialogRequest,
    ) -> GlobalResult<()> {
        if request.snapshot.protocol == SipTransportProtocol::Tcp {
            let association = association.ok_or_else(|| {
                GlobalError::new_sys_error(
                    "TCP restored dialog requires current device association",
                    |msg| error!("{msg}"),
                )
            })?;
            request.snapshot.association_id = self
                .association_ids
                .get(association)
                .map(|entry| *entry.value())
                .ok_or_else(|| {
                    GlobalError::new_sys_error(
                        "TCP association is not registered in native SIP runtime",
                        |msg| error!("{msg}: association={association:?}"),
                    )
                })?;
            request.snapshot.local_addr = association.local_addr;
            request.snapshot.remote_addr = association.remote_addr;
        } else {
            request.snapshot.association_id = 0;
        }
        self.try_send(RuntimeCommand::SendRestoredDialog(request))
    }

    pub fn respond_invite(&self, response: SipInviteResponse) -> GlobalResult<()> {
        self.try_send(RuntimeCommand::RespondInvite(response))
    }

    pub fn send_subscribe(
        &self,
        association: &Association,
        mut subscribe: SipOutboundSubscribe,
    ) -> GlobalResult<()> {
        let protocol = native_protocol(association.protocol)?;
        subscribe.protocol = protocol;
        subscribe.association_id = if protocol == SipTransportProtocol::Tcp {
            self.association_ids
                .get(association)
                .map(|entry| *entry.value())
                .ok_or_else(|| {
                    GlobalError::new_sys_error(
                        "TCP association is not registered in native SIP runtime",
                        |msg| error!("{msg}: association={association:?}"),
                    )
                })?
        } else {
            0
        };
        self.try_send(RuntimeCommand::SendSubscribe(subscribe))
    }

    pub fn close_transport(&self, association: &Association, status: i32) {
        if !matches!(association.protocol, Protocol::TCP) {
            return;
        }
        let Some((_, association_id)) = self.association_ids.remove(association) else {
            return;
        };
        self.queue_transport_close(association_id, status);
    }

    pub fn close_transport_id(&self, association_id: u64, status: i32) -> Option<Association> {
        if association_id == 0 {
            return None;
        }
        let association = self
            .association_ids
            .iter()
            .find_map(|entry| (*entry.value() == association_id).then(|| entry.key().clone()));
        if let Some(association) = &association {
            self.association_ids.remove(association);
        }
        self.queue_transport_close(association_id, status);
        association
    }

    fn queue_transport_close(&self, association_id: u64, status: i32) {
        if let Err(err) = self.try_send(RuntimeCommand::CloseTransport {
            association_id,
            status,
        }) {
            warn!(
                "queue native SIP transport close failed: association_id={association_id}, \
                 err={err}"
            );
        }
    }

    fn try_send(&self, command: RuntimeCommand) -> GlobalResult<()> {
        self.runtime_commands.try_send(command).map_err(|err| {
            GlobalError::new_sys_error(
                &format!("native SIP runtime command queue is unavailable: {err}"),
                |msg| error!("{msg}"),
            )
        })
    }
}

pub struct NativeSipRuntimeService {
    cancel: CancellationToken,
    handle: NativeSipRuntimeHandle,
    auth_task: JoinHandle<()>,
    event_task: JoinHandle<()>,
    runtime_thread: Option<thread::JoinHandle<()>>,
}

impl NativeSipRuntimeService {
    pub fn start(
        advertised_address: Ipv4Addr,
        port: u16,
        realm: String,
        sockets: SipRuntimeSockets,
        auth_cache: Arc<DeviceAuthCache>,
        cancel: CancellationToken,
    ) -> GlobalResult<(Self, mpsc::UnboundedReceiver<SipRuntimeEvent>)> {
        let (lookup_tx, lookup_rx) = mpsc::unbounded_channel();
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (business_tx, business_rx) = mpsc::unbounded_channel();
        let (runtime_command_tx, runtime_command_rx) =
            std_mpsc::sync_channel(RUNTIME_COMMAND_CAPACITY);
        let (startup_tx, startup_rx) = std_mpsc::sync_channel(1);
        let handle = NativeSipRuntimeHandle {
            runtime_commands: runtime_command_tx.clone(),
            association_ids: Arc::new(DashMap::new()),
            next_operation_id: Arc::new(AtomicU64::new(1)),
        };

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
            handle.clone(),
            service_cancel.child_token(),
        ));

        let runtime_cancel = service_cancel.child_token();
        let runtime_thread = thread::Builder::new()
            .name("gmv-pjsip-owner".into())
            .spawn(move || {
                let config = SipRuntimeConfig {
                    advertised_address,
                    port,
                    auth_realm: realm,
                    auth_lookup_timeout: AUTH_LOOKUP_TIMEOUT,
                    max_pending_auth: MAX_PENDING_AUTH,
                    user_agent: format!("Gmv {}", cli_basic().version),
                    ..SipRuntimeConfig::default()
                };
                let (mut runtime, events) = match SipRuntime::start(config, sockets) {
                    Ok(started) => started,
                    Err(err) => {
                        warn!("native SIP owner thread exiting after startup failure: {err}");
                        let _ = startup_tx.send(Err(err.to_string()));
                        return;
                    }
                };
                if startup_tx.send(Ok(())).is_err() {
                    warn!("native SIP owner thread exiting because startup receiver was dropped");
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
                                    SipRuntimeCache::global().complete_native_response(
                                        message.operation_id,
                                        503,
                                        Default::default(),
                                    );
                                }
                            }
                            RuntimeCommand::SendInvite(invite) => {
                                if let Err(err) = runtime.send_invite(&invite) {
                                    warn!(
                                        "send native SIP INVITE failed: operation_id={}, \
                                         err={err}",
                                        invite.operation_id
                                    );
                                    SipRuntimeCache::global()
                                        .fail_native_invite(invite.operation_id, 503);
                                }
                            }
                            RuntimeCommand::SendDialog(request) => {
                                if let Err(err) = runtime.send_dialog_request(&request) {
                                    warn!(
                                        "send native SIP dialog request failed: \
                                         operation_id={}, err={err}",
                                        request.operation_id
                                    );
                                    SipRuntimeCache::global().complete_native_response(
                                        request.operation_id,
                                        503,
                                        Default::default(),
                                    );
                                }
                            }
                            RuntimeCommand::SendRestoredDialog(request) => {
                                if let Err(err) = runtime.send_restored_dialog_request(&request) {
                                    warn!(
                                        "send restored SIP dialog request failed: \
                                         operation_id={}, err={err}",
                                        request.operation_id
                                    );
                                    SipRuntimeCache::global().complete_native_response(
                                        request.operation_id,
                                        503,
                                        Default::default(),
                                    );
                                }
                            }
                            RuntimeCommand::RespondInvite(response) => {
                                if let Err(err) = runtime.respond_invite(&response) {
                                    warn!(
                                        "respond to native SIP INVITE failed: call_id={}, \
                                         status={}, err={err}",
                                        response.call_id, response.status_code
                                    );
                                }
                            }
                            RuntimeCommand::SendSubscribe(subscribe) => {
                                if let Err(err) = runtime.send_subscribe(&subscribe) {
                                    warn!(
                                        "send native SIP SUBSCRIBE failed: operation_id={}, \
                                         err={err}",
                                        subscribe.operation_id
                                    );
                                    if !SipRuntimeCache::global().complete_native_subscription(
                                        subscribe.operation_id,
                                        503,
                                        Default::default(),
                                    ) {
                                        SipRuntimeCache::global().complete_native_response(
                                            subscribe.operation_id,
                                            503,
                                            Default::default(),
                                        );
                                    }
                                }
                            }
                            RuntimeCommand::CloseTransport {
                                association_id,
                                status,
                            } => {
                                if let Err(err) = runtime.close_transport(
                                    association_id,
                                    SipTransportProtocol::Tcp,
                                    status,
                                ) {
                                    warn!(
                                        "close native SIP transport failed: association_id={}, \
                                         err={err}",
                                        association_id
                                    );
                                }
                            }
                        }
                    }

                    if let Err(err) = runtime.poll() {
                        warn!("native SIP owner loop exiting because runtime poll failed: {err}");
                        break;
                    }

                    while let Ok(event) = events.try_recv() {
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
                                    warn!(
                                        "native SIP owner thread exiting because auth lookup receiver was dropped"
                                    );
                                    return;
                                }
                            } else {
                                warn!("native SIP auth event missing lookup identity");
                            }
                        } else {
                            let _ = business_tx.send(event.clone());
                            let _ = event_tx.send(event);
                        }
                    }
                }

                warn!(
                    "native SIP owner loop exited; cancellation_requested={}",
                    runtime_cancel.is_cancelled()
                );
                if let Err(err) = runtime.stop() {
                    warn!("stop native SIP runtime failed: {err}");
                } else {
                    warn!("native SIP runtime stopped");
                }
            })
            .map_err(|err| {
                GlobalError::new_sys_error(
                    &format!("spawn native SIP runtime thread failed: {err}"),
                    |msg| error!("{msg}"),
                )
            })?;

        startup_rx
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
                handle,
                auth_task,
                event_task,
                runtime_thread: Some(runtime_thread),
            },
            event_rx,
        ))
    }

    pub fn send_message(&self, message: SipOutboundMessage) -> GlobalResult<()> {
        self.handle.try_send(RuntimeCommand::SendMessage(message))
    }

    pub fn handle(&self) -> NativeSipRuntimeHandle {
        self.handle.clone()
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        self.cancel.cancel();
        warn!("native SIP runtime service shutdown requested");
        if let Some(thread) = self.runtime_thread.take() {
            match thread.join() {
                Ok(()) => warn!("native SIP owner thread joined"),
                Err(_) => warn!("native SIP owner thread panicked before join"),
            }
        }
        let failed = SipRuntimeCache::global().fail_all_native(503);
        if failed > 0 {
            warn!("failed {failed} native SIP waiter(s) during runtime shutdown");
        }
        self.auth_task.abort();
        self.event_task.abort();
    }
}

impl Drop for NativeSipRuntimeService {
    fn drop(&mut self) {
        self.stop();
    }
}

fn native_protocol(protocol: Protocol) -> GlobalResult<SipTransportProtocol> {
    match protocol {
        Protocol::UDP => Ok(SipTransportProtocol::Udp),
        Protocol::TCP => Ok(SipTransportProtocol::Tcp),
        Protocol::ALL => Err(GlobalError::new_sys_error(
            "protocol ALL cannot be injected into native SIP runtime",
            |msg| error!("{msg}"),
        )),
    }
}

async fn run_native_business_events(
    mut events: mpsc::UnboundedReceiver<SipRuntimeEvent>,
    runtime: NativeSipRuntimeHandle,
    cancel: CancellationToken,
) {
    loop {
        let event = base::tokio::select! {
            event = events.recv() => event,
            _ = cancel.cancelled() => {
                warn!("native SIP business event task exiting after cancellation");
                break;
            },
        };
        let Some(event) = event else {
            warn!("native SIP business event task exiting because event channel closed");
            break;
        };
        if event.kind == SipRuntimeEventKind::TransportClosed {
            runtime.forget_event_association(&event);
        } else {
            runtime.remember_event_association(&event);
        }
        if let Some(operation_id) = event.operation_id {
            match event.kind {
                SipRuntimeEventKind::OutboundResponse => {
                    if let Some(status) = event.status_code {
                        if event.method.as_deref() == Some("INVITE") {
                            SipRuntimeCache::global().complete_native_invite(
                                operation_id,
                                event.call_id.clone().unwrap_or_default(),
                                status,
                                String::from_utf8_lossy(&event.body).into_owned(),
                                event.dialog_snapshot.clone(),
                            );
                        } else {
                            let metadata = response_metadata(&event);
                            if event.method.as_deref() == Some("SUBSCRIBE") {
                                if !SipRuntimeCache::global().complete_native_subscription(
                                    operation_id,
                                    status,
                                    metadata.clone(),
                                ) {
                                    SipRuntimeCache::global().complete_native_response(
                                        operation_id,
                                        status,
                                        metadata,
                                    );
                                }
                            } else {
                                SipRuntimeCache::global().complete_native_response(
                                    operation_id,
                                    status,
                                    metadata,
                                );
                            }
                        }
                    }
                }
                SipRuntimeEventKind::RuntimeFault => {
                    if !SipRuntimeCache::global().fail_native_invite(operation_id, 503) {
                        if !SipRuntimeCache::global().complete_native_subscription(
                            operation_id,
                            503,
                            Default::default(),
                        ) {
                            SipRuntimeCache::global().complete_native_response(
                                operation_id,
                                503,
                                Default::default(),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        let message_event = match GbMessageEvent::from_native(&event) {
            Ok(event) => event,
            Err(err) => {
                warn!("parse native SIP MESSAGE failed: {err}");
                None
            }
        };
        let business_event = GbRegisterEvent::from_native(&event)
            .map(GbSipEvent::Register)
            .or_else(|| message_event.map(GbSipEvent::Message))
            .or_else(|| GbIncomingInviteEvent::from_native(&event).map(GbSipEvent::IncomingInvite))
            .or_else(|| native_dialog_event(&event));
        if let Some(business_event) = business_event {
            if let GbSipEvent::IncomingInvite(invite) = &business_event
                && !SipRuntimeCache::global().complete_broadcast_invite(invite)
            {
                if let Err(err) = runtime.respond_invite(SipInviteResponse {
                    call_id: invite.call_id.clone(),
                    status_code: 501,
                    reason: Some("Inbound session is not supported".into()),
                    content_type: None,
                    body: Vec::new(),
                }) {
                    warn!(
                        "queue unsupported incoming INVITE response failed: call_id={}, err={err}",
                        invite.call_id
                    );
                }
            }
            if let Err(err) = apply_business_event(business_event) {
                warn!("apply native SIP business event failed: {err}");
            }
        }
    }
}

impl NativeSipRuntimeHandle {
    fn remember_event_association(&self, event: &SipRuntimeEvent) {
        let Some(association_id) = event.association_id else {
            return;
        };
        let Some(protocol) = event.protocol.and_then(session_protocol) else {
            return;
        };
        let (Some(local_addr), Some(remote_addr)) = (event.local_addr, event.remote_addr) else {
            return;
        };
        self.association_ids.insert(
            Association {
                local_addr,
                remote_addr,
                protocol,
            },
            association_id,
        );
    }

    fn forget_event_association(&self, event: &SipRuntimeEvent) {
        let Some(association_id) = event.association_id else {
            return;
        };
        let Some(protocol) = event.protocol.and_then(session_protocol) else {
            return;
        };
        let (Some(local_addr), Some(remote_addr)) = (event.local_addr, event.remote_addr) else {
            return;
        };
        let association = Association {
            local_addr,
            remote_addr,
            protocol,
        };
        if self
            .association_ids
            .remove_if(&association, |_, current_id| *current_id == association_id)
            .is_some()
        {
            Register::detach_device_association(&association);
        }
    }
}

fn session_protocol(protocol: SipTransportProtocol) -> Option<Protocol> {
    match protocol {
        SipTransportProtocol::Udp => Some(Protocol::UDP),
        SipTransportProtocol::Tcp => Some(Protocol::TCP),
        SipTransportProtocol::Tls => None,
    }
}

fn native_dialog_event(event: &SipRuntimeEvent) -> Option<GbSipEvent> {
    if event.kind != SipRuntimeEventKind::RequestReceived {
        return None;
    }
    match event.method.as_deref()? {
        "BYE" => Some(GbSipEvent::Bye(GbByeEvent {
            call_id: event.call_id.clone()?,
            stream_id: event
                .call_id
                .as_deref()
                .and_then(|call_id| SipRuntimeCache::global().stream_id_by_call_id(call_id)),
            device_id: None,
        })),
        "CANCEL" => Some(GbSipEvent::Cancel {
            call_id: event.call_id.clone()?,
        }),
        "ACK" => Some(GbSipEvent::Ack {
            call_id: event.call_id.clone()?,
        }),
        _ => None,
    }
}

fn response_metadata(event: &SipRuntimeEvent) -> super::runtime_cache::SipResponseMetadata {
    super::runtime_cache::SipResponseMetadata {
        call_id: event.call_id.clone(),
        cseq: event.cseq,
        event: event.event.clone(),
        contact: event.contact.clone(),
        record_routes: Vec::new(),
        from_header: event.from_header.clone(),
        to_header: event.to_header.clone(),
        to_tag: event
            .to_header
            .as_deref()
            .and_then(gmv_pjsip::message::extract_tag),
        expires: event.expires_seconds,
    }
}

async fn run_auth_batches(
    mut lookups: mpsc::UnboundedReceiver<AuthLookup>,
    runtime_commands: std_mpsc::SyncSender<RuntimeCommand>,
    auth_cache: Arc<DeviceAuthCache>,
    cancel: CancellationToken,
) {
    loop {
        let first = base::tokio::select! {
            lookup = lookups.recv() => lookup,
            _ = cancel.cancelled() => {
                warn!("native SIP auth task exiting after cancellation");
                break;
            },
        };
        let Some(first) = first else {
            warn!("native SIP auth task exiting because lookup channel closed");
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
                        .try_send(RuntimeCommand::CompleteAuth(AuthCompletion {
                            lookup_id: lookup.lookup_id,
                            result,
                        }))
                        .is_err()
                    {
                        warn!("native SIP auth task exiting because runtime command queue closed");
                        return;
                    }
                }
            }
            Err(err) => {
                error!("batch native SIP auth lookup failed: {err}");
                for lookup in batch {
                    if runtime_commands
                        .try_send(RuntimeCommand::CompleteAuth(AuthCompletion {
                            lookup_id: lookup.lookup_id,
                            result: SipAuthLookupResult::Reject,
                        }))
                        .is_err()
                    {
                        warn!("native SIP auth task exiting because runtime command queue closed");
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
    use std::net::{Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use std::sync::atomic::AtomicU64;
    use std::sync::mpsc as std_mpsc;
    use std::time::Duration;

    use base::bytes::Bytes;
    use base::dashmap::DashMap;
    use base::net::state::{Association, Protocol};
    use base::tokio::runtime::Runtime;
    use base::tokio::time;
    use base::tokio_util::sync::CancellationToken;
    use gmv_pjsip::{
        SipAuthLookupResult, SipOutboundMessage, SipRuntimeEvent, SipRuntimeEventKind,
        SipTransportProtocol,
    };

    use super::{NativeSipRuntimeHandle, NativeSipRuntimeService, RUNTIME_TEST_LOCK, auth_result};
    use crate::gb::sip::auth::DeviceAuthCache;
    use crate::storage::entity::GmvOauth;

    fn header_value<'a>(message: &'a str, name: &str) -> &'a str {
        message
            .lines()
            .find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.eq_ignore_ascii_case(name).then_some(value.trim())
            })
            .unwrap_or_else(|| panic!("missing {name}"))
    }

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

    fn transport_closed_event(association: &Association, association_id: u64) -> SipRuntimeEvent {
        SipRuntimeEvent {
            event_id: 0,
            kind: SipRuntimeEventKind::TransportClosed,
            protocol: Some(SipTransportProtocol::Tcp),
            status_code: None,
            pj_status: 0,
            method: None,
            call_id: None,
            cseq: None,
            content_type: None,
            body: Vec::new(),
            local_addr: Some(association.local_addr),
            remote_addr: Some(association.remote_addr),
            lookup_id: None,
            device_id: None,
            realm: None,
            expires_seconds: None,
            contact: None,
            user_agent: None,
            gb_version: None,
            operation_id: None,
            association_id: Some(association_id),
            from_header: None,
            to_header: None,
            subject: None,
            event: None,
            subscription_state: None,
            dialog_snapshot: None,
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
    fn delayed_transport_close_does_not_remove_current_association_id() {
        let (runtime_commands, _commands) = std_mpsc::sync_channel(1);
        let handle = NativeSipRuntimeHandle {
            runtime_commands,
            association_ids: Arc::new(DashMap::new()),
            next_operation_id: Arc::new(AtomicU64::new(1)),
        };
        let association = Association::new(
            "127.0.0.1:5060".parse().unwrap(),
            "127.0.0.1:40000".parse().unwrap(),
            Protocol::TCP,
        );
        handle.association_ids.insert(association.clone(), 2);

        handle.forget_event_association(&transport_closed_event(&association, 1));
        assert_eq!(
            handle.association_ids.get(&association).map(|id| *id),
            Some(2)
        );

        handle.forget_event_association(&transport_closed_event(&association, 2));
        assert!(!handle.association_ids.contains_key(&association));
    }
}
