use crate::gb::depot::SipPackage;
use crate::state::runner::Runner;
use crate::state::schedule;
use crate::state::schedule::ScheduleTask;
use base::cfg_lib::conf;
use base::cfg_lib::conf::{CheckFromConf, FieldCheckError};
use base::chrono::Local;
use base::constructor::Get;
use base::dbx::mysqlx::get_conn_by_pool;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::error;
use base::net::state::{Zip, CHANNEL_BUFFER_SIZE};
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

#[derive(Debug, Get, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.session", check)]
pub struct SessionInfo {
    domain: String,
    domain_id: String,
    http_source: String,
    lan_ip: Ipv4Addr,
    wan_ip: Ipv4Addr,
    lan_port: u16,
    wan_port: u16,
}
impl CheckFromConf for SessionInfo {
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

impl ScheduleTask for SessionInfo{
    fn do_something(&self) -> Pin<Box<dyn Future<Output=()> + Send + '_>> {
        Box::pin(async move {
            let _ = self.heart_to_db().await;
        })
    }
}
impl Runner for SessionInfo {
    fn next() -> impl Future<Output=()> + Send {
        async {
            let conf = SessionInfo::get_session_by_conf();
            let _ = conf.init_to_db().await;
            tokio::time::sleep(Duration::from_secs(60)).await;
            let schedule = Schedule::from_str("0 */1 * * * *").expect("heart_to_db cron invalid");
            let tx = schedule::get_schedule_tx();
            let _ = tx.send((schedule, Arc::new(conf))).await.hand_log(|msg| error!("{msg}"));
        }

    }
}
impl SessionInfo {
    async fn heart_server(){
        let conf = SessionInfo::get_session_by_conf();
        let _ = conf.init_to_db().await;

    }
    async fn heart_to_db(&self)->GlobalResult<()>{
        let pool = get_conn_by_pool();
        let _ = sqlx::query(r#"update GB_SERVER set heart_time=? where domain_id=?"#)
            .bind(Local::now().naive_local())
            .bind(&self.domain_id)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }
    async fn init_to_db(&self)->GlobalResult<()>{
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
            .bind(60)
            .execute(pool)
            .await.hand_log(|msg| error!("{msg}"))?;
        Ok(())
    }

    pub fn get_session_by_conf() -> Self {
        SessionInfo::conf()
    }

    pub fn listen_gb_server(&self) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
        let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", self.get_wan_port()))
            .hand_log(|msg| error! {"{msg}"})?;
        let res = net::sdx::listen(net::state::Protocol::ALL, socket_addr);
        res
    }

    pub async fn run(
        tu: (Option<std::net::TcpListener>, Option<UdpSocket>),
        cancel_token: CancellationToken,
    ) -> GlobalResult<()> {
        let (output, input) = net::sdx::run_by_tokio(tu).await?;
        let (sip_pkg_tx, sip_pkg_rx) = mpsc::channel::<SipPackage>(CHANNEL_BUFFER_SIZE);
        RWContext::init(output.clone(), sip_pkg_tx.clone());
        let handle = Handle::current();
        handle.spawn(SessionInfo::next());
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
