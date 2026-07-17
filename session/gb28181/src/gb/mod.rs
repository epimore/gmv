use crate::register::core::Register;
use crate::storage::db_task;
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::log::{info, warn};
use base::net;
use base::serde::Deserialize;
use base::tokio::runtime::Handle;
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::{SipRuntimeSockets, SipTransportProtocol};
use regex::Regex;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::time::{Duration, Instant};

use crate::storage::entity::GmvDevice;

pub mod sip;

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.session", check)]
pub struct SessionConf {
    pub domain: String,
    pub domain_id: String,
    pub lan_ip: Ipv4Addr,
    pub wan_ip: Ipv4Addr,
    pub lan_port: u16,
    pub wan_port: u16,
}
impl CheckFromConf for SessionConf {
    fn _field_check(&self) -> Result<(), FieldCheckError> {
        let re = Regex::new(r"^\d{20}$").unwrap();
        if !re.is_match(&self.domain_id) {
            return Err(FieldCheckError::BizError(format!(
                "domain_id must be 20 digits: {}",
                self.domain_id
            )));
        }
        Ok(())
    }
}
impl SessionConf {
    pub fn get_session_by_conf() -> Self {
        SessionConf::conf()
    }

    pub fn media_receiver_id(&self) -> &str {
        &self.domain_id
    }

    pub fn listen_gb_server(&self) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
        let socket_addr = SocketAddr::from((self.lan_ip, self.wan_port));
        net::listen(net::state::Protocol::ALL, socket_addr)
    }

    pub async fn run(
        tu: (Option<std::net::TcpListener>, Option<UdpSocket>),
        cancel_token: CancellationToken,
    ) -> GlobalResult<()> {
        crate::storage::db::initialize().await?;
        db_task::init(cancel_token.child_token());
        let session_conf = SessionConf::get_session_by_conf();
        crate::storage::ssrc_sequence::SsrcSequence::initialize(&session_conf.domain_id).await?;
        let auth_cache = sip::auth::init_global().await?;
        let sockets = SipRuntimeSockets {
            tcp: tu.0,
            udp: tu.1,
            tls: None,
        };
        let native_service = sip::NativeSipRuntimeService::start(
            session_conf.wan_ip,
            session_conf.wan_port,
            session_conf.domain.clone(),
            sockets,
            auth_cache.clone(),
            cancel_token.child_token(),
        )?;
        let native_runtime = native_service.handle();
        native_runtime.install_global()?;
        Register::init(cancel_token.child_token())?;
        install_restart_recovery_sources(&native_runtime).await?;
        let handle = Handle::current();
        handle.spawn(crate::service::dialog_recovery::run_startup_recovery());
        handle.spawn(sip::auth::run_cleanup_task(cancel_token.child_token()));
        handle.spawn(sip::run_cleanup_task(cancel_token.child_token()));
        let native_shutdown = cancel_token.child_token();
        handle.spawn(async move {
            native_shutdown.cancelled().await;
            native_service.shutdown();
        });
        Ok(())
    }
}

const RECOVERY_SOURCE_PAGE_SIZE: u32 = 1_000;

#[derive(Default)]
struct RecoveryBootstrapReport {
    scanned: usize,
    eligible: usize,
    installed: usize,
    expired: usize,
    invalid: usize,
}

enum RecoveryCandidate {
    Eligible(sip::NativeRecoverySource),
    Expired,
    Invalid,
}

async fn install_restart_recovery_sources(
    native_runtime: &sip::NativeSipRuntimeHandle,
) -> GlobalResult<()> {
    let mut cursor: Option<String> = None;
    let mut report = RecoveryBootstrapReport::default();

    loop {
        let page =
            GmvDevice::page_recovery_candidates(cursor.as_deref(), RECOVERY_SOURCE_PAGE_SIZE)
                .await?;
        if page.scanned == 0 {
            break;
        }
        report.scanned += page.scanned;
        report.invalid += page.invalid;
        cursor = page.next_device_id;

        let now = base::chrono::Local::now().naive_local();
        let monotonic_now = Instant::now();
        let mut sources = Vec::with_capacity(page.devices.len());
        for device in &page.devices {
            match recovery_source_from_device(device, now, monotonic_now) {
                RecoveryCandidate::Eligible(source) => sources.push(source),
                RecoveryCandidate::Expired => report.expired += 1,
                RecoveryCandidate::Invalid => report.invalid += 1,
            }
        }
        report.eligible += sources.len();
        report.installed += native_runtime.allow_recovery_sources(sources).await?;

        if page.scanned < RECOVERY_SOURCE_PAGE_SIZE as usize {
            break;
        }
    }

    if report.invalid > 0 {
        warn!(
            "GB28181 restart recovery candidates skipped: invalid={}",
            report.invalid
        );
    }
    info!(
        "GB28181 restart recovery sources initialized: scanned={}, eligible={}, installed={}, expired={}, invalid={}",
        report.scanned, report.eligible, report.installed, report.expired, report.invalid
    );
    Ok(())
}

fn recovery_source_from_device(
    device: &GmvDevice,
    now: base::chrono::NaiveDateTime,
    monotonic_now: Instant,
) -> RecoveryCandidate {
    if device.device_id.is_empty()
        || device.device_id.len() >= 128
        || device.device_id.as_bytes().contains(&0)
    {
        return RecoveryCandidate::Invalid;
    }
    let register_deadline =
        device.register_time + base::chrono::Duration::seconds(i64::from(device.register_expires));
    let Some(online_deadline) = device.online_expire_time else {
        return RecoveryCandidate::Invalid;
    };
    let recovery_deadline = register_deadline.min(online_deadline);
    let remaining_ms = recovery_deadline
        .signed_duration_since(now)
        .num_milliseconds();
    if remaining_ms <= 0 {
        return RecoveryCandidate::Expired;
    }
    let protocol = if device.transport.eq_ignore_ascii_case("UDP") {
        SipTransportProtocol::Udp
    } else if device.transport.eq_ignore_ascii_case("TCP") {
        SipTransportProtocol::Tcp
    } else {
        return RecoveryCandidate::Invalid;
    };
    let Ok(remote_addr) = device.local_addr.parse::<SocketAddr>() else {
        return RecoveryCandidate::Invalid;
    };
    let ttl_ms = u64::try_from(remaining_ms)
        .unwrap_or(u64::MAX)
        .min(u64::from(u32::MAX));
    RecoveryCandidate::Eligible(sip::NativeRecoverySource {
        device_id: device.device_id.clone(),
        remote_address: remote_addr.ip().to_string(),
        protocol,
        deadline: monotonic_now + Duration::from_millis(ttl_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::{RecoveryCandidate, recovery_source_from_device};
    use crate::storage::entity::GmvDevice;
    use base::chrono::{Duration as TimeDelta, Local};
    use gmv_pjsip::SipTransportProtocol;
    use std::time::{Duration, Instant};

    #[test]
    fn recovery_source_uses_shorter_lease_and_registered_source_ip() {
        let now = Local::now().naive_local();
        let monotonic_now = Instant::now();
        let device = GmvDevice {
            device_id: "34020000001320000001".to_string(),
            transport: "UDP".to_string(),
            register_expires: 120,
            register_time: now - TimeDelta::seconds(30),
            online_expire_time: Some(now + TimeDelta::seconds(20)),
            local_addr: "192.0.2.10:5060".to_string(),
            ..GmvDevice::default()
        };

        let RecoveryCandidate::Eligible(source) =
            recovery_source_from_device(&device, now, monotonic_now)
        else {
            panic!("expected eligible recovery source");
        };
        assert_eq!(source.protocol, SipTransportProtocol::Udp);
        assert_eq!(source.remote_address, "192.0.2.10");
        assert!(source.deadline >= monotonic_now + Duration::from_secs(19));
        assert!(source.deadline <= monotonic_now + Duration::from_secs(20));
    }

    #[test]
    fn recovery_source_rejects_expired_and_invalid_snapshots() {
        let now = Local::now().naive_local();
        let monotonic_now = Instant::now();
        let expired = GmvDevice {
            device_id: "expired".to_string(),
            transport: "UDP".to_string(),
            register_expires: 10,
            register_time: now - TimeDelta::seconds(11),
            online_expire_time: Some(now + TimeDelta::seconds(20)),
            local_addr: "192.0.2.10:5060".to_string(),
            ..GmvDevice::default()
        };
        assert!(matches!(
            recovery_source_from_device(&expired, now, monotonic_now),
            RecoveryCandidate::Expired
        ));

        let invalid = GmvDevice {
            register_time: now,
            register_expires: 60,
            online_expire_time: Some(now + TimeDelta::seconds(20)),
            transport: "TLS".to_string(),
            local_addr: "not-an-address".to_string(),
            ..GmvDevice::default()
        };
        assert!(matches!(
            recovery_source_from_device(&invalid, now, monotonic_now),
            RecoveryCandidate::Invalid
        ));
    }
}
