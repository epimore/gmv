use crate::register::core::Register;
use crate::storage::db_task;
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::net;
use base::serde::Deserialize;
use base::tokio::runtime::Handle;
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::SipRuntimeSockets;
use regex::Regex;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;

pub mod sip;

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.session", check)]
pub struct SessionConf {
    pub domain: String,
    pub domain_id: String,
    pub http_source: String,
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
        let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", self.wan_port))
            .hand_log(|msg| error! {"{msg}"})?;
        let res = net::listen(net::state::Protocol::ALL, socket_addr);
        res
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
        let (native_service, _native_events) = sip::NativeSipRuntimeService::start(
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
