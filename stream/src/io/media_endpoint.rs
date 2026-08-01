use std::collections::{HashMap, HashSet};
use std::net::{TcpListener, UdpSocket};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult};
use base::log::{debug, error};
use base::net::rw::{ManagedPacketIo, PacketWriter, U16BeLengthPrefixEncoder};
use base::tokio::sync::Mutex;
use base::tokio::time::{self, MissedTickBehavior};
use base::utils::rt::GlobalRuntime;
use gmv_protocol::common::v1::{Endpoint, EndpointMode};

use crate::general::cfg::{MediaListenerConf, MediaListenerMode};
use crate::io::rtp_handler::{self, EndpointDispatchContext};
use crate::state::register::Register;

pub type BoundMediaListener = (Option<TcpListener>, Option<UdpSocket>);

pub enum MediaBootstrap {
    Single { listener: BoundMediaListener },
    Multi,
}

#[derive(Clone)]
pub struct MediaEndpointLease {
    pub endpoint_id: String,
    pub stream_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub generation: u64,
    pub port: u16,
    pub writer: PacketWriter<U16BeLengthPrefixEncoder>,
}

impl MediaEndpointLease {
    pub fn endpoint(&self, advertised_host: &str) -> Endpoint {
        Endpoint {
            name: "rtp".to_string(),
            scheme: "rtp".to_string(),
            host: advertised_host.to_string(),
            port: u32::from(self.port),
            mode: EndpointMode::Single as i32,
            labels: HashMap::from([
                ("endpoint_id".to_string(), self.endpoint_id.clone()),
                ("generation".to_string(), self.generation.to_string()),
            ]),
        }
    }
}

pub struct ReserveMediaEndpoint {
    pub stream_id: String,
    pub lease_id: String,
    pub route_id: String,
    pub expected_ssrc: Option<u32>,
    pub reservation_ttl: Option<Duration>,
    pub confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MediaEndpointState {
    Listening,
    Releasing,
}

struct MediaEndpointRecord {
    endpoint_id: String,
    stream_id: String,
    lease_id: String,
    route_id: String,
    generation: u64,
    port: u16,
    state: MediaEndpointState,
    permanent: bool,
    deadline: Option<Instant>,
    dispatch: Arc<EndpointDispatchContext>,
    io: Arc<ManagedPacketIo<U16BeLengthPrefixEncoder>>,
}

impl MediaEndpointRecord {
    fn lease(&self) -> MediaEndpointLease {
        MediaEndpointLease {
            endpoint_id: self.endpoint_id.clone(),
            stream_id: self.stream_id.clone(),
            lease_id: self.lease_id.clone(),
            route_id: self.route_id.clone(),
            generation: self.generation,
            port: self.port,
            writer: self.io.writer(),
        }
    }
}

#[derive(Default)]
struct MediaEndpointStateStore {
    next_generation: u64,
    next_port_offset: usize,
    endpoints: HashMap<String, MediaEndpointRecord>,
    stream_index: HashMap<String, String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MediaEndpointStats {
    pub total: usize,
    pub free: usize,
    pub listening: usize,
    pub confirmed: usize,
    pub releasing: usize,
    pub bind_failures: u64,
    pub exhaustions: u64,
}

pub struct MediaEndpointManager {
    runtime: GlobalRuntime,
    conf: MediaListenerConf,
    state: Mutex<MediaEndpointStateStore>,
    bind_failures: AtomicU64,
    exhaustions: AtomicU64,
}

static MEDIA_ENDPOINT_MANAGER: OnceLock<Arc<MediaEndpointManager>> = OnceLock::new();

#[cfg(test)]
static NEXT_TEST_PORT: std::sync::atomic::AtomicU16 = std::sync::atomic::AtomicU16::new(30_000);

#[cfg(test)]
pub(crate) fn find_free_test_range(count: u16) -> crate::general::cfg::MediaPortRange {
    use std::net::Ipv4Addr;
    use std::sync::atomic::Ordering;

    let seed = NEXT_TEST_PORT.fetch_add(count.saturating_add(2), Ordering::Relaxed);
    for start in seed..60_000u16.saturating_sub(count) {
        let mut holders = Vec::new();
        let mut available = true;
        for port in start..start + count {
            match (
                TcpListener::bind((Ipv4Addr::LOCALHOST, port)),
                UdpSocket::bind((Ipv4Addr::LOCALHOST, port)),
            ) {
                (Ok(tcp), Ok(udp)) => holders.push((tcp, udp)),
                _ => {
                    available = false;
                    break;
                }
            }
        }
        if available {
            drop(holders);
            return crate::general::cfg::MediaPortRange {
                start,
                end: start + count - 1,
            };
        }
    }
    panic!("no free test port range");
}

impl MediaEndpointManager {
    pub fn new(
        runtime: GlobalRuntime,
        conf: MediaListenerConf,
        bootstrap: MediaBootstrap,
    ) -> GlobalResult<Arc<Self>> {
        let manager = Arc::new(Self {
            runtime,
            conf,
            state: Mutex::new(MediaEndpointStateStore {
                next_generation: 1,
                ..MediaEndpointStateStore::default()
            }),
            bind_failures: AtomicU64::new(0),
            exhaustions: AtomicU64::new(0),
        });
        match bootstrap {
            MediaBootstrap::Single { listener }
                if manager.conf.mode == MediaListenerMode::Single =>
            {
                manager.install_single(listener)?;
            }
            MediaBootstrap::Multi if manager.conf.mode == MediaListenerMode::Multi => {}
            _ => {
                return Err(GlobalError::new_sys_error(
                    "media bootstrap does not match configured listener mode",
                    |msg| error!("{msg}"),
                ));
            }
        }
        Ok(manager)
    }

    fn install_single(&self, listener: BoundMediaListener) -> GlobalResult<()> {
        let endpoint_id = format!("single-{}", self.conf.single_port);
        let dispatch = Arc::new(EndpointDispatchContext::new(
            endpoint_id.clone(),
            1,
            None,
            None,
            false,
        ));
        let io = Arc::new(rtp_handler::start_managed(
            &self.runtime,
            format!("stream-media-{endpoint_id}"),
            listener,
            self.runtime.cancel.child_token(),
            dispatch.clone(),
        )?);
        let mut state = self.state.try_lock().map_err(|_| {
            GlobalError::new_sys_error("media endpoint state is unexpectedly locked", |msg| {
                error!("{msg}")
            })
        })?;
        state.endpoints.insert(
            endpoint_id.clone(),
            MediaEndpointRecord {
                endpoint_id,
                stream_id: String::new(),
                lease_id: String::new(),
                route_id: String::new(),
                generation: 1,
                port: self.conf.single_port,
                state: MediaEndpointState::Listening,
                permanent: true,
                deadline: None,
                dispatch,
                io,
            },
        );
        state.next_generation = 2;
        Ok(())
    }

    pub fn install_global(manager: Arc<Self>) -> GlobalResult<()> {
        MEDIA_ENDPOINT_MANAGER.set(manager).map_err(|_| {
            GlobalError::new_sys_error("media endpoint manager already initialized", |msg| {
                error!("{msg}")
            })
        })
    }

    pub fn global() -> Option<&'static Arc<Self>> {
        MEDIA_ENDPOINT_MANAGER.get()
    }

    pub fn mode(&self) -> MediaListenerMode {
        self.conf.mode
    }

    pub async fn owns_active_lease(&self, stream_id: &str, lease_id: &str) -> bool {
        if self.conf.mode == MediaListenerMode::Single {
            return true;
        }
        let state = self.state.lock().await;
        state
            .stream_index
            .get(stream_id)
            .and_then(|endpoint_id| state.endpoints.get(endpoint_id))
            .is_some_and(|endpoint| {
                endpoint.lease_id == lease_id && endpoint.state == MediaEndpointState::Listening
            })
    }

    pub fn capability_endpoint(&self) -> Endpoint {
        let (port, mode, labels) = match self.conf.mode {
            MediaListenerMode::Single => {
                (self.conf.single_port, EndpointMode::Single, HashMap::new())
            }
            MediaListenerMode::Multi => (
                self.conf.port_range.start,
                EndpointMode::Multi,
                HashMap::from([
                    (
                        "port_range_start".to_string(),
                        self.conf.port_range.start.to_string(),
                    ),
                    (
                        "port_range_end".to_string(),
                        self.conf.port_range.end.to_string(),
                    ),
                    ("protocols".to_string(), "tcp,udp".to_string()),
                    ("allocation".to_string(), "dynamic".to_string()),
                ]),
            ),
        };
        Endpoint {
            name: "rtp".to_string(),
            scheme: "rtp".to_string(),
            host: self.conf.advertised_host.clone(),
            port: u32::from(port),
            mode: mode as i32,
            labels,
        }
    }

    pub async fn reserve(&self, request: ReserveMediaEndpoint) -> GlobalResult<MediaEndpointLease> {
        if request.stream_id.is_empty()
            || request.lease_id.is_empty()
            || request.route_id.is_empty()
        {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "stream_id, lease_id and route_id are required",
                |msg| error!("{msg}"),
            ));
        }
        let mut state = self.state.lock().await;
        if self.conf.mode == MediaListenerMode::Single {
            return state
                .endpoints
                .values()
                .find(|endpoint| endpoint.permanent)
                .map(MediaEndpointRecord::lease)
                .ok_or_else(|| {
                    GlobalError::new_sys_error("single media endpoint is unavailable", |msg| {
                        error!("{msg}")
                    })
                });
        }
        let expected_ssrc = request
            .expected_ssrc
            .filter(|ssrc| *ssrc != 0)
            .ok_or_else(|| {
                GlobalError::new_biz_error(
                    BaseErrorCode::InvalidRequest.code(),
                    "expected_ssrc is required in multi mode",
                    |msg| error!("{msg}: stream_id={}", request.stream_id),
                )
            })?;
        if let Some(endpoint_id) = state.stream_index.get(&request.stream_id).cloned() {
            let existing = state
                .endpoints
                .get_mut(&endpoint_id)
                .expect("indexed endpoint");
            if existing.lease_id == request.lease_id {
                if existing.route_id != request.route_id
                    || existing.dispatch.expected_ssrc != Some(expected_ssrc)
                {
                    return Err(GlobalError::new_biz_error(
                        BaseErrorCode::InvalidState.code(),
                        "media endpoint idempotency key conflicts with the existing route or SSRC",
                        |msg| error!("{msg}: stream_id={}", request.stream_id),
                    ));
                }
                if existing.state == MediaEndpointState::Releasing {
                    return Err(GlobalError::new_biz_error(
                        BaseErrorCode::InvalidState.code(),
                        "media endpoint is releasing",
                        |msg| error!("{msg}: stream_id={}", request.stream_id),
                    ));
                }
                if request.confirmed {
                    existing.deadline = None;
                }
                return Ok(existing.lease());
            }
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::AlreadyExists.code(),
                "stream already owns a different media endpoint lease",
                |msg| error!("{msg}: stream_id={}", request.stream_id),
            ));
        }

        let ports = self.port_candidates(state.next_port_offset);
        let active_ports = state
            .endpoints
            .values()
            .map(|endpoint| endpoint.port)
            .collect::<HashSet<_>>();
        for (offset, port) in ports {
            if active_ports.contains(&port) {
                continue;
            }
            let listener = match rtp_handler::listen_media_server(self.conf.bind_ip, port) {
                Ok(listener) => listener,
                Err(error) => {
                    self.bind_failures.fetch_add(1, Ordering::Relaxed);
                    debug!("media endpoint bind candidate failed: port={port}, error={error}");
                    continue;
                }
            };
            let generation = state.next_generation;
            state.next_generation = state.next_generation.checked_add(1).ok_or_else(|| {
                GlobalError::new_sys_error("media endpoint generation counter exhausted", |msg| {
                    error!("{msg}")
                })
            })?;
            let endpoint_id = format!("media-{port}-{generation}");
            let dispatch = Arc::new(EndpointDispatchContext::new(
                endpoint_id.clone(),
                generation,
                Some(request.stream_id.clone()),
                Some(expected_ssrc),
                true,
            ));
            let io = Arc::new(rtp_handler::start_managed(
                &self.runtime,
                format!("stream-media-{endpoint_id}"),
                listener,
                self.runtime.cancel.child_token(),
                dispatch.clone(),
            )?);
            let ttl = request
                .reservation_ttl
                .unwrap_or_else(|| Duration::from_secs(self.conf.reservation_timeout_secs));
            let deadline = (!request.confirmed).then(|| Instant::now() + ttl);
            let record = MediaEndpointRecord {
                endpoint_id: endpoint_id.clone(),
                stream_id: request.stream_id.clone(),
                lease_id: request.lease_id,
                route_id: request.route_id,
                generation,
                port,
                state: MediaEndpointState::Listening,
                permanent: false,
                deadline,
                dispatch,
                io,
            };
            let lease = record.lease();
            state
                .stream_index
                .insert(request.stream_id.clone(), endpoint_id.clone());
            state.endpoints.insert(endpoint_id, record);
            let port_count = usize::from(self.conf.port_range.end - self.conf.port_range.start) + 1;
            state.next_port_offset = (offset + 1) % port_count;
            Register::bind_media_endpoint(
                &request.stream_id,
                lease.endpoint_id.clone(),
                lease.generation,
                expected_ssrc,
            );
            return Ok(lease);
        }
        self.exhaustions.fetch_add(1, Ordering::Relaxed);
        Err(GlobalError::new_biz_error(
            BaseErrorCode::IoBusy.code(),
            "media port pool is exhausted",
            |msg| {
                error!(
                    "{msg}: range={}-{}",
                    self.conf.port_range.start, self.conf.port_range.end
                )
            },
        ))
    }

    fn port_candidates(&self, start_offset: usize) -> Vec<(usize, u16)> {
        let count = usize::from(self.conf.port_range.end - self.conf.port_range.start) + 1;
        (0..count)
            .map(|step| {
                let offset = (start_offset + step) % count;
                (offset, self.conf.port_range.start + offset as u16)
            })
            .collect()
    }

    pub async fn release(&self, stream_id: &str, lease_id: &str) -> GlobalResult<bool> {
        let endpoint = {
            let mut state = self.state.lock().await;
            let Some(endpoint_id) = state.stream_index.get(stream_id).cloned() else {
                return Ok(false);
            };
            let endpoint = state
                .endpoints
                .get_mut(&endpoint_id)
                .expect("indexed endpoint");
            if endpoint.lease_id != lease_id {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::InvalidState.code(),
                    "media endpoint lease does not match",
                    |msg| error!("{msg}: stream_id={stream_id}"),
                ));
            }
            endpoint.state = MediaEndpointState::Releasing;
            endpoint.dispatch.deactivate();
            (endpoint_id, endpoint.generation, endpoint.io.clone())
        };
        endpoint.2.close_and_wait().await?;
        let mut state = self.state.lock().await;
        if state
            .endpoints
            .get(&endpoint.0)
            .is_some_and(|record| record.generation == endpoint.1)
        {
            state.endpoints.remove(&endpoint.0);
            state.stream_index.remove(stream_id);
            Register::unbind_media_endpoint(stream_id, &endpoint.0, endpoint.1);
        }
        Ok(true)
    }

    pub async fn release_stream(&self, stream_id: &str) -> GlobalResult<bool> {
        let lease_id = {
            let state = self.state.lock().await;
            let Some(endpoint_id) = state.stream_index.get(stream_id) else {
                return Ok(false);
            };
            state.endpoints[endpoint_id].lease_id.clone()
        };
        self.release(stream_id, &lease_id).await
    }

    pub fn release_stream_detached(stream_id: &str) {
        let Some(manager) = Self::global().cloned() else {
            return;
        };
        if manager.conf.mode == MediaListenerMode::Single {
            return;
        }
        let stream_id = stream_id.to_string();
        let task_stream_id = stream_id.clone();
        let runtime = manager.runtime.clone();
        let result = runtime.spawn("stream-media-release", async move {
            if let Err(error) = manager.release_stream(&task_stream_id).await {
                error!("media endpoint release failed: stream_id={task_stream_id}, error={error}");
            }
        });
        if let Err(error) = result {
            debug!("media endpoint release task rejected: stream_id={stream_id}, error={error}");
        }
    }

    pub async fn expire_once(&self) {
        let candidates = {
            let state = self.state.lock().await;
            let now = Instant::now();
            state
                .endpoints
                .values()
                .filter(|endpoint| {
                    !endpoint.permanent
                        && endpoint.state == MediaEndpointState::Listening
                        && !endpoint.dispatch.is_observed()
                        && endpoint.deadline.is_some_and(|deadline| deadline <= now)
                })
                .map(|endpoint| (endpoint.stream_id.clone(), endpoint.lease_id.clone()))
                .collect::<Vec<_>>()
        };
        for (stream_id, lease_id) in candidates {
            let endpoint = {
                let mut state = self.state.lock().await;
                let Some(endpoint_id) = state.stream_index.get(&stream_id).cloned() else {
                    continue;
                };
                let endpoint = state
                    .endpoints
                    .get_mut(&endpoint_id)
                    .expect("indexed endpoint");
                if endpoint.lease_id != lease_id
                    || endpoint.state != MediaEndpointState::Listening
                    || endpoint.dispatch.is_observed()
                    || !endpoint
                        .deadline
                        .is_some_and(|deadline| deadline <= Instant::now())
                {
                    continue;
                }
                endpoint.state = MediaEndpointState::Releasing;
                endpoint.dispatch.deactivate();
                (endpoint_id, endpoint.generation, endpoint.io.clone())
            };
            if let Err(error) = endpoint.2.close_and_wait().await {
                error!(
                    "expired media endpoint release failed: stream_id={stream_id}, error={error}"
                );
                continue;
            }
            let mut state = self.state.lock().await;
            if state
                .endpoints
                .get(&endpoint.0)
                .is_some_and(|record| record.generation == endpoint.1)
            {
                state.endpoints.remove(&endpoint.0);
                state.stream_index.remove(&stream_id);
                Register::unbind_media_endpoint(&stream_id, &endpoint.0, endpoint.1);
            }
        }
    }

    pub fn spawn_expiry_task(manager: Arc<Self>) -> GlobalResult<()> {
        let runtime = manager.runtime.clone();
        let cancel = runtime.cancel.clone();
        runtime.spawn("stream-media-endpoint-expiry", async move {
            let mut interval = time::interval(Duration::from_secs(1));
            interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            loop {
                base::tokio::select! {
                    _ = cancel.cancelled() => break,
                    _ = interval.tick() => manager.expire_once().await,
                }
            }
        })?;
        Ok(())
    }

    pub async fn stats(&self) -> MediaEndpointStats {
        let state = self.state.lock().await;
        self.stats_from_state(&state)
    }

    pub fn stats_snapshot(&self) -> MediaEndpointStats {
        self.state
            .try_lock()
            .map(|state| self.stats_from_state(&state))
            .unwrap_or_else(|_| MediaEndpointStats {
                total: self.capacity(),
                bind_failures: self.bind_failures.load(Ordering::Relaxed),
                exhaustions: self.exhaustions.load(Ordering::Relaxed),
                ..MediaEndpointStats::default()
            })
    }

    fn stats_from_state(&self, state: &MediaEndpointStateStore) -> MediaEndpointStats {
        let total = self.capacity();
        let mut stats = MediaEndpointStats {
            total,
            bind_failures: self.bind_failures.load(Ordering::Relaxed),
            exhaustions: self.exhaustions.load(Ordering::Relaxed),
            ..MediaEndpointStats::default()
        };
        for endpoint in state.endpoints.values() {
            match endpoint.state {
                MediaEndpointState::Releasing => stats.releasing += 1,
                MediaEndpointState::Listening
                    if endpoint.permanent
                        || endpoint.deadline.is_none()
                        || endpoint.dispatch.is_observed() =>
                {
                    stats.confirmed += 1
                }
                MediaEndpointState::Listening => stats.listening += 1,
            }
        }
        stats.free = stats
            .total
            .saturating_sub(stats.listening + stats.confirmed + stats.releasing);
        stats
    }

    fn capacity(&self) -> usize {
        match self.conf.mode {
            MediaListenerMode::Single => 1,
            MediaListenerMode::Multi => {
                usize::from(self.conf.port_range.end - self.conf.port_range.start) + 1
            }
        }
    }

    pub async fn shutdown(&self) -> GlobalResult<()> {
        let endpoints = {
            let mut state = self.state.lock().await;
            state
                .endpoints
                .values_mut()
                .map(|endpoint| {
                    endpoint.state = MediaEndpointState::Releasing;
                    endpoint.dispatch.deactivate();
                    (
                        endpoint.stream_id.clone(),
                        endpoint.endpoint_id.clone(),
                        endpoint.generation,
                        endpoint.io.clone(),
                    )
                })
                .collect::<Vec<_>>()
        };
        let mut close_error = None;
        for endpoint in endpoints {
            match endpoint.3.close_and_wait().await {
                Ok(_) if !endpoint.0.is_empty() => {
                    Register::unbind_media_endpoint(&endpoint.0, &endpoint.1, endpoint.2)
                }
                Ok(_) => {}
                Err(err) => {
                    error!(
                        "media endpoint shutdown failed: endpoint_id={}, generation={}, error={err}",
                        endpoint.1, endpoint.2
                    );
                    if close_error.is_none() {
                        close_error = Some(err);
                    }
                }
            }
        }
        if let Some(err) = close_error {
            return Err(err);
        }
        let mut state = self.state.lock().await;
        state.endpoints.clear();
        state.stream_index.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{MediaBootstrap, MediaEndpointManager, ReserveMediaEndpoint, find_free_test_range};
    use crate::general::cfg::{MediaListenerConf, MediaListenerMode, MediaPortRange};
    use crate::io::rtp_handler;
    use base::utils::rt::GlobalRuntime;
    use std::net::{IpAddr, Ipv4Addr, TcpListener, UdpSocket};
    use std::time::Duration;

    #[cfg(unix)]
    const SIGTERM_HELPER_ENV: &str = "GMV_STREAM_MEDIA_SIGTERM_HELPER";

    fn multi_conf(range: MediaPortRange) -> MediaListenerConf {
        MediaListenerConf {
            mode: MediaListenerMode::Multi,
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            advertised_host: Ipv4Addr::LOCALHOST.to_string(),
            single_port: 0,
            port_range: range,
            reservation_timeout_secs: 30,
        }
    }

    fn single_conf(port: u16) -> MediaListenerConf {
        MediaListenerConf {
            mode: MediaListenerMode::Single,
            bind_ip: IpAddr::V4(Ipv4Addr::LOCALHOST),
            advertised_host: Ipv4Addr::LOCALHOST.to_string(),
            single_port: port,
            port_range: MediaPortRange::default(),
            reservation_timeout_secs: 30,
        }
    }

    fn request(stream_id: &str, lease_id: &str, ssrc: u32) -> ReserveMediaEndpoint {
        ReserveMediaEndpoint {
            stream_id: stream_id.to_string(),
            lease_id: lease_id.to_string(),
            route_id: format!("route-{stream_id}"),
            expected_ssrc: Some(ssrc),
            reservation_ttl: None,
            confirmed: false,
        }
    }

    #[tokio::test]
    async fn multi_reservation_is_idempotent_and_isolates_ports() {
        let range = find_free_test_range(2);
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            multi_conf(range),
            MediaBootstrap::Multi,
        )
        .unwrap();

        let first = manager
            .reserve(request("stream-a", "lease-a", 1001))
            .await
            .unwrap();
        let repeated = manager
            .reserve(request("stream-a", "lease-a", 1001))
            .await
            .unwrap();
        let second = manager
            .reserve(request("stream-b", "lease-b", 1002))
            .await
            .unwrap();

        assert_eq!(first.endpoint_id, repeated.endpoint_id);
        assert_eq!(first.generation, repeated.generation);
        assert_ne!(first.port, second.port);
        let mut conflicting_idempotent = request("stream-a", "lease-a", 1001);
        conflicting_idempotent.route_id = "route-other".to_string();
        assert!(manager.reserve(conflicting_idempotent).await.is_err());
        assert!(
            manager
                .reserve(request("stream-a", "lease-other", 1001))
                .await
                .is_err()
        );
        let mut missing_ssrc = request("stream-c", "lease-c", 1003);
        missing_ssrc.expected_ssrc = None;
        assert!(manager.reserve(missing_ssrc).await.is_err());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn single_mode_keeps_one_permanent_shared_listener() {
        let range = find_free_test_range(1);
        let listener =
            rtp_handler::listen_media_server(IpAddr::V4(Ipv4Addr::LOCALHOST), range.start).unwrap();
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            single_conf(range.start),
            MediaBootstrap::Single { listener },
        )
        .unwrap();

        let first = manager
            .reserve(request("stream-a", "lease-a", 1001))
            .await
            .unwrap();
        let second = manager
            .reserve(request("stream-b", "lease-b", 1002))
            .await
            .unwrap();
        assert_eq!(first.endpoint_id, second.endpoint_id);
        assert_eq!(first.port, range.start);
        assert!(!manager.release("stream-a", "lease-a").await.unwrap());
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, range.start)).is_err());

        manager.shutdown().await.unwrap();
        let rebound_tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, range.start)).unwrap();
        let rebound_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, range.start)).unwrap();
        drop((rebound_tcp, rebound_udp));
    }

    #[tokio::test]
    async fn release_waits_then_reuses_port_with_new_generation() {
        let range = find_free_test_range(1);
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            multi_conf(range),
            MediaBootstrap::Multi,
        )
        .unwrap();
        let first = manager
            .reserve(request("stream-a", "lease-a", 1001))
            .await
            .unwrap();
        assert!(
            manager
                .reserve(request("stream-b", "lease-b", 1002))
                .await
                .is_err()
        );
        assert_eq!(manager.stats().await.exhaustions, 1);

        assert!(manager.release("stream-a", "lease-a").await.unwrap());
        let rebound_tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, first.port)).unwrap();
        let rebound_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, first.port)).unwrap();
        drop((rebound_tcp, rebound_udp));

        let second = manager
            .reserve(request("stream-b", "lease-b", 1002))
            .await
            .unwrap();
        assert_eq!(first.port, second.port);
        assert!(second.generation > first.generation);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn allocation_skips_an_externally_occupied_candidate() {
        let range = find_free_test_range(2);
        let occupied = UdpSocket::bind((Ipv4Addr::LOCALHOST, range.start)).unwrap();
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            multi_conf(range),
            MediaBootstrap::Multi,
        )
        .unwrap();

        let endpoint = manager
            .reserve(request("stream-a", "lease-a", 1001))
            .await
            .unwrap();
        assert_eq!(endpoint.port, range.end);
        assert_eq!(manager.stats().await.bind_failures, 1);

        manager.shutdown().await.unwrap();
        drop(occupied);
    }

    #[tokio::test]
    async fn unobserved_reservation_expires_and_releases_listener() {
        let range = find_free_test_range(1);
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            multi_conf(range),
            MediaBootstrap::Multi,
        )
        .unwrap();
        let mut reserve = request("stream-a", "lease-a", 1001);
        reserve.reservation_ttl = Some(Duration::from_millis(10));
        let endpoint = manager.reserve(reserve).await.unwrap();

        base::tokio::time::sleep(Duration::from_millis(20)).await;
        manager.expire_once().await;

        let stats = manager.stats().await;
        assert_eq!(stats.free, 1);
        let rebound_tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
        let rebound_udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, endpoint.port)).unwrap();
        drop((rebound_tcp, rebound_udp));
    }

    #[tokio::test]
    async fn confirmed_reservation_is_not_reclaimed_by_ttl() {
        let range = find_free_test_range(1);
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            multi_conf(range),
            MediaBootstrap::Multi,
        )
        .unwrap();
        let mut reserve = request("talk-a", "talk-lease-a", 1001);
        reserve.reservation_ttl = Some(Duration::from_millis(1));
        reserve.confirmed = true;
        let endpoint = manager.reserve(reserve).await.unwrap();

        base::tokio::time::sleep(Duration::from_millis(5)).await;
        manager.expire_once().await;

        assert_eq!(manager.stats().await.confirmed, 1);
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, endpoint.port)).is_err());
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn repeated_confirmed_reservation_clears_original_ttl() {
        let range = find_free_test_range(1);
        let manager = MediaEndpointManager::new(
            GlobalRuntime::get_main_runtime(),
            multi_conf(range),
            MediaBootstrap::Multi,
        )
        .unwrap();
        let mut reserve = request("talk-a", "lease-a", 1001);
        reserve.reservation_ttl = Some(Duration::from_millis(1));
        manager.reserve(reserve).await.unwrap();

        let mut confirmed = request("talk-a", "lease-a", 1001);
        confirmed.confirmed = true;
        manager.reserve(confirmed).await.unwrap();
        base::tokio::time::sleep(Duration::from_millis(5)).await;
        manager.expire_once().await;

        assert_eq!(manager.stats().await.confirmed, 1);
        manager.shutdown().await.unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn active_media_endpoint_sigterm_helper() {
        use std::io::Write;

        if std::env::var_os(SIGTERM_HELPER_ENV).is_none() {
            return;
        }
        let runtime = GlobalRuntime::register_default(base::utils::rt::RuntimeType::CommonNetwork)
            .expect("register stream helper runtime");
        let async_runtime = base::tokio::runtime::Runtime::new().expect("create async runtime");
        let range = {
            let seed = 50_000 + (std::process::id() % 5_000) as u16;
            let port = (seed..59_999)
                .find(|port| {
                    let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, *port));
                    let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, *port));
                    tcp.is_ok() && udp.is_ok()
                })
                .expect("find helper media port");
            MediaPortRange {
                start: port,
                end: port,
            }
        };
        let manager =
            MediaEndpointManager::new(runtime.clone(), multi_conf(range), MediaBootstrap::Multi)
                .expect("create media endpoint manager");
        let endpoint = async_runtime
            .block_on(manager.reserve(request("stream-sigterm", "lease-sigterm", 1001)))
            .expect("reserve active media endpoint");
        let shutdown_manager = manager.clone();
        let shutdown_cancel = runtime.cancel.clone();
        runtime
            .spawn("stream-media-sigterm-shutdown", async move {
                shutdown_cancel.cancelled().await;
                shutdown_manager
                    .shutdown()
                    .await
                    .expect("shutdown endpoints");
            })
            .expect("spawn endpoint shutdown");
        println!("READY {}", endpoint.port);
        std::io::stdout().flush().expect("flush helper readiness");

        let report = GlobalRuntime::order_shutdown(&[base::utils::rt::RuntimeType::CommonNetwork]);
        assert_eq!(report.signal, base::daemon::signal::ExitSignal::Terminate);
        assert!(report.is_graceful());
    }

    #[cfg(unix)]
    #[test]
    fn sigterm_closes_active_media_endpoint_before_process_exit() {
        use std::io::{BufRead, BufReader};
        use std::process::{Command, Stdio};
        use std::sync::mpsc;
        use std::time::Instant;

        if std::env::var_os(SIGTERM_HELPER_ENV).is_some() {
            return;
        }
        let mut child = Command::new(std::env::current_exe().expect("current test executable"))
            .args([
                "--exact",
                "io::media_endpoint::tests::active_media_endpoint_sigterm_helper",
                "--nocapture",
            ])
            .env(SIGTERM_HELPER_ENV, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn stream SIGTERM helper");
        let stdout = child.stdout.take().expect("helper stdout");
        let (line_tx, line_rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if line_tx.send(line).is_err() {
                    break;
                }
            }
        });
        let ready_deadline = Instant::now() + Duration::from_secs(5);
        let port = loop {
            let remaining = ready_deadline.saturating_duration_since(Instant::now());
            assert!(!remaining.is_zero(), "helper did not become ready");
            let line = line_rx
                .recv_timeout(remaining)
                .expect("read helper readiness");
            if let Some(port) = line.strip_prefix("READY ") {
                break port.parse::<u16>().expect("helper media port");
            }
        };
        let status = Command::new("kill")
            .args(["-TERM", &child.id().to_string()])
            .status()
            .expect("send SIGTERM to helper");
        assert!(status.success());

        let exit_deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = child.try_wait().expect("wait for helper") {
                assert!(status.success(), "helper exited with {status}");
                break;
            }
            if Instant::now() >= exit_deadline {
                child.kill().expect("kill stuck helper");
                let _ = child.wait();
                panic!("helper did not exit after SIGTERM");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let tcp = TcpListener::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        let udp = UdpSocket::bind((Ipv4Addr::LOCALHOST, port)).unwrap();
        drop((tcp, udp));
    }
}
