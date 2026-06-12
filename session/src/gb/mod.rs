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
mod sip_tcp_splitter;

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
        if re.is_match(&self.domain_id) {
            return Ok(());
        }
        Err(FieldCheckError::BizError(format!(
            "domain_id must be 20 digits: {}",
            self.domain_id
        )))
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
        let (output, input) = io::rw_by_tokio(tu, cancel_token.child_token())?;
        db_task::init(cancel_token.child_token());
        let session_conf = SessionConf::get_session_by_conf();
        let auth_cache = sip::auth::init_global().await?;
        Register::init(
            session_conf.clone(),
            output.clone(),
            cancel_token.child_token(),
        )?;
        sip::GbSipRuntime::init_global(sip::GbSipConfig::from_session_conf(
            &session_conf,
            auth_cache,
        ));
        RWContext::init(output.clone());
        let handle = Handle::current();
        handle.spawn(SessionConf::heart_server());
        handle.spawn(sip::auth::run_cleanup_task(cancel_token.child_token()));
        handle.spawn(sip::run_cleanup_task(cancel_token.child_token()));
        base::tokio::spawn(io::read(input, output.clone(), cancel_token.child_token()));
        let output_sender = output.clone();
        base::tokio::spawn(async move {
            cancel_token.cancelled().await;
            let _ = output_sender.send(Zip::build_shutdown_zip(None)).await;
        });
        Ok(())
    }
}
