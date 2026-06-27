use crate::register::core::Register;
use crate::register::core::SERVER_HEART_SECOND;
use crate::storage::{db, db_task};
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::chrono::Local;
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
use std::sync::Arc;

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
    async fn heart_server() {
        let conf = SessionConf::get_session_by_conf();
        let _ = conf.init_to_db().await;
        let _ = Register::server_keep_heart_update_db(Arc::from(conf.domain_id)).await;
    }
    pub async fn heart_to_db(&self) -> GlobalResult<()> {
        let _ = db::execute!(
            r#"update GB_SERVER set heart_time=? where domain_id=?"#,
            Local::now().naive_local(),
            &self.domain_id,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
    async fn init_to_db(&self) -> GlobalResult<()> {
        let sql = match db::backend() {
            db::SessionDatabaseBackend::Mysql => {
                r#"insert into GB_SERVER (domain_id,domain,sip_ip,
        sip_port,http_source,status,heart_time,heart_cycle) values (?,?,?,?,?,?,?,?)
        ON DUPLICATE KEY UPDATE domain=VALUES(domain),sip_ip=VALUES(sip_ip),
        sip_port=VALUES(sip_port),http_source=VALUES(http_source),status=VALUES(status),heart_time=VALUES(heart_time),heart_cycle=VALUES(heart_cycle)"#
            }
            db::SessionDatabaseBackend::Sqlite => {
                r#"insert into GB_SERVER (domain_id,domain,sip_ip,
        sip_port,http_source,status,heart_time,heart_cycle) values (?,?,?,?,?,?,?,?)
        ON CONFLICT(domain_id) DO UPDATE SET domain=excluded.domain,sip_ip=excluded.sip_ip,
        sip_port=excluded.sip_port,http_source=excluded.http_source,status=excluded.status,heart_time=excluded.heart_time,heart_cycle=excluded.heart_cycle"#
            }
        };
        db::execute!(
            sql,
            &self.domain_id,
            &self.domain,
            self.wan_ip.to_string(),
            i64::from(self.wan_port),
            &self.http_source,
            1_i64,
            Local::now().naive_local(),
            SERVER_HEART_SECOND as i64,
        )
        .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

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
        Register::init(session_conf.clone(), cancel_token.child_token())?;
        let handle = Handle::current();
        handle.spawn(crate::service::dialog_recovery::run_startup_recovery());
        handle.spawn(SessionConf::heart_server());
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
