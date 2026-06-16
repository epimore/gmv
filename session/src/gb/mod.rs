use crate::register::core::Register;
use crate::register::core::SERVER_HEART_SECOND;
use crate::storage::db_task;
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::chrono::Local;
use base::dbx::mysqlx::get_conn_by_pool;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::net;
use base::net::state::Zip;
use base::serde::Deserialize;
use base::tokio::runtime::Handle;
use base::tokio_util::sync::CancellationToken;
pub use core::rw::RWContext;
use regex::Regex;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;
use std::sync::Arc;

mod core;
mod io;
pub mod sip;

#[derive(Clone, Debug, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.session", check)]
pub struct SessionConf {
    pub domain: String,
    pub domain_id: String,
    #[serde(default)]
    pub media_server_id: Option<String>,
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
        if let Some(media_server_id) = &self.media_server_id {
            if !re.is_match(media_server_id) {
                return Err(FieldCheckError::BizError(format!(
                    "media_server_id must be 20 digits: {media_server_id}"
                )));
            }
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
        let pool = get_conn_by_pool();
        let _ = sqlx::query(r#"update GB_SERVER set heart_time=? where domain_id=?"#)
            .bind(Local::now().naive_local())
            .bind(&self.domain_id)
            .execute(pool)
            .await
            .hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
    async fn init_to_db(&self) -> GlobalResult<()> {
        let pool = get_conn_by_pool();
        sqlx::query(r#"insert into GB_SERVER (domain_id,domain,sip_ip,
        sip_port,http_source,status,heart_time,heart_cycle) values (?,?,?,?,?,?,?,?)
        ON DUPLICATE KEY UPDATE domain_id=VALUES(domain_id),domain=VALUES(domain),sip_ip=VALUES(sip_ip),
        sip_port=VALUES(sip_port),http_source=VALUES(http_source),status=VALUES(status),heart_time=VALUES(heart_time),heart_cycle=VALUES(heart_cycle)"#)
            .bind(&self.domain_id)
            .bind(&self.domain)
            .bind(&self.wan_ip)
            .bind(&self.wan_port)
            .bind(&self.http_source)
            .bind(1)
            .bind(Local::now().naive_local())
            .bind(SERVER_HEART_SECOND)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub fn get_session_by_conf() -> Self {
        SessionConf::conf()
    }

    pub fn media_receiver_id(&self) -> &str {
        self.media_server_id.as_deref().unwrap_or(&self.domain_id)
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
        let io::NativeSessionIo {
            output,
            input,
            writer,
            close_tracker,
        } = io::rw_by_tokio_native(tu, cancel_token.child_token())?;
        db_task::init(cancel_token.child_token());
        let session_conf = SessionConf::get_session_by_conf();
        let auth_cache = sip::auth::init_global().await?;
        let (native_service, _native_events, native_transmits) =
            sip::NativeSipRuntimeService::start(
                session_conf.wan_ip,
                session_conf.wan_port,
                session_conf.domain.clone(),
                auth_cache.clone(),
                cancel_token.child_token(),
            )?;
        let native_runtime = native_service.handle();
        native_runtime.install_global()?;
        Register::init(
            session_conf.clone(),
            output.clone(),
            cancel_token.child_token(),
        )?;
        let handle = Handle::current();
        handle.spawn(SessionConf::heart_server());
        handle.spawn(sip::auth::run_cleanup_task(cancel_token.child_token()));
        handle.spawn(sip::run_cleanup_task(cancel_token.child_token()));
        handle.spawn(io::write_native_net(
            native_transmits,
            writer,
            native_runtime.clone(),
            close_tracker,
            cancel_token.child_token(),
        ));
        handle.spawn(io::read_native(
            input,
            output.clone(),
            native_runtime,
            cancel_token.child_token(),
        ));
        let native_shutdown = cancel_token.child_token();
        handle.spawn(async move {
            native_shutdown.cancelled().await;
            native_service.shutdown();
        });
        let output_sender = output.clone();
        base::tokio::spawn(async move {
            cancel_token.cancelled().await;
            let _ = output_sender.send(Zip::build_shutdown_zip(None)).await;
        });
        Ok(())
    }
}
