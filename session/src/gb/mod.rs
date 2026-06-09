use crate::gb::depot::SipPackage;
use crate::register::core::Register;
use crate::register::core::SERVER_HEART_SECOND;
use crate::state::runner::Runner;
use crate::state::schedule;
use crate::state::schedule::ScheduleTask;
use crate::storage::db_task;
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::chrono::Local;
use base::dbx::mysqlx::get_conn_by_pool;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::net::state::{CHANNEL_BUFFER_SIZE, Zip};
use base::serde::Deserialize;
use base::tokio::runtime::Handle;
use base::tokio::sync::{mpsc, oneshot};
use base::tokio_util::sync::CancellationToken;
use base::{net, tokio};
pub use core::rw::RWContext;
use cron::Schedule;
use regex::Regex;
use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

mod core;
pub mod depot;
pub mod handler;
mod io;
mod sip_tcp_splitter;
mod sip;

#[derive(Debug, Deserialize)]
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
//
// impl ScheduleTask for SessionConf {
//     fn do_something(&self) -> Pin<Box<dyn Future<Output=()> + Send + '_>> {
//         Box::pin(async move {
//             let _ = self.heart_to_db().await;
//         })
//     }
// }
// impl Runner for SessionConf {
//     fn next() -> impl Future<Output=()> + Send {
//         async {
//             let conf = SessionConf::get_session_by_conf();
//             let _ = conf.init_to_db().await;
//             tokio::time::sleep(Duration::from_secs(60)).await;
//             let schedule = Schedule::from_str("0 */1 * * * *").expect("heart_to_db cron invalid");
//             let tx = schedule::get_schedule_tx();
//             let _ = tx.send((schedule, Arc::new(conf))).await.hand_log(|msg| error!("{msg}"));
//         }
//
//     }
// }
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
        let (sip_pkg_tx, sip_pkg_rx) = mpsc::channel::<SipPackage>(CHANNEL_BUFFER_SIZE);
        db_task::init(cancel_token.child_token());
        Register::init(
            SessionConf::get_session_by_conf(),
            output.clone(),
            sip_pkg_tx.clone(),
            cancel_token.child_token(),
        )?;
        RWContext::init(output.clone(), sip_pkg_tx.clone());
        let handle = Handle::current();
        handle.spawn(SessionConf::heart_server());
        let ctx = Arc::new(depot::DepotContext::init(
            handle.clone(),
            cancel_token.clone(),
            output.clone(),
        ));
        base::tokio::spawn(io::read(
            input,
            output.clone(),
            sip_pkg_tx,
            cancel_token.child_token(),
            ctx.clone(),
        ));
        let output_sender = output.clone();
        base::tokio::spawn(io::write(
            sip_pkg_rx,
            output,
            cancel_token.child_token(),
            ctx,
        ));
        base::tokio::spawn(async move {
            cancel_token.cancelled().await;
            let _ = output_sender.send(Zip::build_shutdown_zip(None)).await;
        });
        Ok(())
    }
}
