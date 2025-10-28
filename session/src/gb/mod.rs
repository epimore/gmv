use std::net::{Ipv4Addr, SocketAddr, TcpListener, UdpSocket};
use std::str::FromStr;

use base::cfg_lib::conf;
use base::constructor::Get;
use base::serde::Deserialize;
use base::tokio::sync::mpsc;

use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{error, info};
use base::net;
use base::net::state::{CHANNEL_BUFFER_SIZE, Zip};
use base::tokio::select;
use base::tokio_util::sync::CancellationToken;
pub use core::rw::RWSession;

mod core;
pub mod handler;
mod io;
mod sip_tcp_splitter;

#[derive(Debug, Get, Deserialize)]
#[serde(crate = "base::serde")]
#[conf(prefix = "server.session")]
pub struct SessionInfo {
    lan_ip: Ipv4Addr,
    wan_ip: Ipv4Addr,
    lan_port: u16,
    wan_port: u16,
}

impl SessionInfo {
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
        let (output_tx, output_rx) = mpsc::channel(CHANNEL_BUFFER_SIZE);
        base::tokio::spawn(io::read(input, output_tx, cancel_token.child_token()));
        let output_sender = output.clone();
        base::tokio::spawn(io::write(output_rx, output, cancel_token.child_token()));
        base::tokio::spawn(async move {
            cancel_token.cancelled().await;
            let _ = output_sender.send(Zip::build_shutdown_zip(None)).await;
        });
        Ok(())
    }
}
