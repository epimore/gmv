use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;

use base::serde::Deserialize;
use base::tokio::sync::mpsc;
use base::cfg_lib::conf;
use base::constructor::Get;

use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, info};
use base::net;
use base::net::state::{CHANNEL_BUFFER_SIZE};

pub use crate::gb::shared::rw::RWSession;

mod shared;
pub mod handler;
mod io;

#[derive(Debug, Get, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.session")]
pub struct SessionConf {
    lan_ip: Ipv4Addr,
    wan_ip: Ipv4Addr,
    lan_port: u16,
    wan_port: u16,
}

impl SessionConf {
    pub fn get_session_by_conf() -> Self {
        SessionConf::conf()
    }

    pub fn listen_gb_server(&self) -> GlobalResult<(Option<TcpListener>, Option<UdpSocket>)> {
        let socket_addr = SocketAddr::from_str(&format!("0.0.0.0:{}", self.get_wan_port())).hand_log(|msg| error! {"{msg}"})?;
        let res = net::sdx::listen(net::state::Protocol::ALL, socket_addr);
        info!("Listen to gb28181 session over tcp and udp,listen: 0.0.0.0:{}; wan ip: {}", self.get_wan_port(), self.get_wan_ip());
        res
    }

    pub async fn run(tu: (Option<std::net::TcpListener>, Option<UdpSocket>)) -> GlobalResult<()> {
        let (output, input) = net::sdx::run_by_tokio(tu).await?;
        let (output_tx, output_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        let read_task = base::tokio::spawn(async move {
            io::read(input, output_tx).await;
        });
        let write_task = base::tokio::spawn(async move {
            io::write(output_rx, output).await;
        });
        read_task.await.hand_log(|msg| error!("读取数据异常:{msg}"))?;
        write_task.await.hand_log(|msg| error!("写出数据异常:{msg}"))?;
        Ok(())
    }
}

