use std::net::UdpSocket;
use log::{error, info};
use common::daemon::Daemon;
use common::exception::{GlobalError, GlobalResult, TransError};
use common::{logger, tokio};
use common::tokio::sync::mpsc;
use crate::general::mode::ServerConf;
use crate::io::{http_handler, rtp_handler};
use crate::state::cache;
use crate::trans;

pub struct App {
    conf: ServerConf,
}

impl Daemon<(std::net::TcpListener, (Option<std::net::TcpListener>, Option<UdpSocket>))> for App {
    fn init_privilege() -> GlobalResult<(Self, (std::net::TcpListener, (Option<std::net::TcpListener>, Option<UdpSocket>)))>
    where
        Self: Sized,
    {
        let app = App {
            conf: cache::get_server_conf().clone()
        };
        logger::Logger::init()?;
        banner();
        let http_listener = http_handler::listen_http_server(*(app.conf.get_http_port()))?;
        let tu = rtp_handler::listen_gb_server(*(app.conf.get_rtp_port()))?;
        Ok((app, (http_listener, tu)))
    }

    fn run_app(self, t: (std::net::TcpListener, (Option<std::net::TcpListener>, Option<UdpSocket>))) -> GlobalResult<()> {
        let (http_listener, tu) = t;
        let conf = self.conf;
        let node_name = conf.get_name().clone();
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                let st = tokio::spawn(async move {
                    info!("Stream server start running...");
                    rtp_handler::run(tu).await?;
                    error!("Stream server stop");
                    Ok::<(), GlobalError>(())
                });
                let (tx, rx) = mpsc::channel(100);
                let web = tokio::spawn(async move {
                    info!("Web server start running...");
                    http_handler::run(node_name, http_listener, tx).await?;
                    error!("Web server stop");
                    Ok::<(), GlobalError>(())
                });
                let tr = tokio::spawn(async move {
                    info!("Trans server start running...");
                    trans::run(rx).await;
                    info!("Trans server start running...");
                });
                st.await.hand_log(|msg| error!("Stream:{msg}"))??;
                web.await.hand_log(|msg| error!("WEB:{msg}"))??;
                tr.await.hand_log(|msg| error!("Trans:{msg}"))?;
                Ok::<(), GlobalError>(())
            })?;
        error!("系统异常退出...");
        Ok(())
    }
}

fn banner() {
    let br = r#"
            ___   __  __  __   __    _      ___    _____    ___     ___     ___   __  __
    o O O  / __| |  \/  | \ \ / /   (_)    / __|  |_   _|  | _ \   | __|   /   \ |  \/  |
   o      | (_ | | |\/| |  \ V /     _     \__ \    | |    |   /   | _|    | - | | |\/| |
  oO__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   _|_|_   |_|_\   |___|   |_|_| |_|__|_|
 {======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""T""|_|""R""|_|""E""|_|""A""|_|""M""|
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'
"#;
    info!("{}", br);
}