use std::net::UdpSocket;

use common::daemon::Daemon;
use common::dbx::mysqlx;
use common::exception::{GlobalError, GlobalResult, TransError};
use common::log::{error, info};
use common::logger;
use common::tokio;

use crate::gb::SessionConf;
use crate::general::http::Http;

#[derive(Debug)]
pub struct AppInfo {
    session_conf: SessionConf,
    http: Http,
}

impl Daemon<(std::net::TcpListener, (Option<std::net::TcpListener>, Option<UdpSocket>))> for AppInfo {
    fn init_privilege() -> GlobalResult<(Self, (std::net::TcpListener, (Option<std::net::TcpListener>, Option<UdpSocket>)))>
    where
        Self: Sized,
    {
        let app_info = AppInfo {
            session_conf: SessionConf::get_session_by_conf(),
            http: Http::get_http_by_conf(),
        };
        logger::Logger::init()?;
        banner();
        let http_listener = app_info.http.listen_http_server()?;
        let tu = app_info.session_conf.listen_gb_server()?;
        Ok((app_info, (http_listener, tu)))
    }

    fn run_app(self, t: (std::net::TcpListener, (Option<std::net::TcpListener>, Option<UdpSocket>))) -> GlobalResult<()> {
        let http = self.http;
        let (http_listener, tu) = t;
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(async {
                mysqlx::init_conn_pool()?;
                let web = tokio::spawn(async move {
                    info!("Web server start running...");
                    http.run(http_listener).await?;
                    error!("Web server stop");
                    Ok::<(), GlobalError>(())
                });
                let se = tokio::spawn(async move {
                    info!("Session server start running...");
                    SessionConf::run(tu).await?;
                    error!("Session server stop");
                    Ok::<(), GlobalError>(())
                });
                se.await.hand_log(|msg| error!("Session:{msg}"))??;
                web.await.hand_log(|msg| error!("WEB:{msg}"))??;
                Ok::<(), GlobalError>(())
            })?;
        error!("系统异常退出...");
        Ok(())
    }
}

fn banner() {
    let br = r#"
            ___   __  __  __   __    _      ___     ___     ___     ___     ___     ___    _  _
    o O O  / __| |  \/  | \ \ / /   (_)    / __|   | __|   / __|   / __|   |_ _|   / _ \  | \| |
   o      | (_ | | |\/| |  \ V /     _     \__ \   | _|    \__ \   \__ \    | |   | (_) | | .` |
  o0__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   |___|   |___/   |___/   |___|   \___/  |_|\_|
 {======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""E""|_|""S""|_|""S""|_|""I""|_|""O""|_|""N""|
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'
"#;
    info!("{}", br);
}