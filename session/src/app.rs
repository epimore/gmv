use std::net::UdpSocket;
use base::cfg_lib::{default_cli_basic, CliBasic};
use crate::gb::SessionInfo;
use crate::http::Http;
use crate::state::runner::{PicsRunner, Runner};
use base::daemon::Daemon;
use base::exception::GlobalResult;
use base::log::info;
use base::logger;
use base::utils::rt::{GlobalRuntime, RuntimeType};

#[derive(Debug)]
pub struct AppInfo {
    session_conf: SessionInfo,
    http: Http,
}

impl
    Daemon<(
        std::net::TcpListener,
        (Option<std::net::TcpListener>, Option<UdpSocket>),
    )> for AppInfo
{
    fn cli_basic() -> CliBasic {
        default_cli_basic!()
    }

    fn init_privilege() -> GlobalResult<(
        Self,
        (
            std::net::TcpListener,
            (Option<std::net::TcpListener>, Option<UdpSocket>),
        ),
    )>
    where
        Self: Sized,
    {
        let app_info = AppInfo {
            session_conf: SessionInfo::get_session_by_conf(),
            http: Http::get_http_by_conf(),
        };
        logger::Logger::init()?;
        let http_listener = app_info.http.listen_http_server()?;
        let tu = app_info.session_conf.listen_gb_server()?;
        banner(app_info.http.port, *app_info.session_conf.get_wan_port(), |msg| info!("{msg}"));
        Ok((app_info, (http_listener, tu)))
    }

    fn run_app(
        self,
        t: (
            std::net::TcpListener,
            (Option<std::net::TcpListener>, Option<UdpSocket>),
        ),
    ) -> GlobalResult<()> {
        let http = self.http;
        let (http_listener, tu) = t;
        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        network_rt.rt_handle.spawn(SessionInfo::run(tu, network_rt.cancel.clone()));
        network_rt
            .rt_handle
            .spawn(async move { http.run(http_listener, network_rt.cancel.clone()).await });
        GlobalRuntime::get_main_runtime().rt_handle.spawn(PicsRunner::next());
        GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork], |msg| info!("{msg}"));
        Ok(())
    }
}

fn banner<F: FnOnce(String)>(http_port: u16, rtp_port: u16, f: F) {
    let msg = format!(
        r#"
            ___   __  __  __   __    _      ___     ___     ___     ___     ___     ___     _  _
    o O O  / __| |  \/  | \ \ / /   (_)    / __|   | __|   / __|   / __|   |_ _|   / _ \   | \| |
   o      | (_ | | |\/| |  \ V /     _     \__ \   | _|    \__ \   \__ \    | |   | (_) |  | .` |
  o0__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   |___|   |___/   |___/   |___|   \___/   |_|\_|
 [======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""E""|_|""S""|_|""S""|_|""I""|_|""O""|_|""N""|==]
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'

┌──────────────────┬──────────────────┬──────────────┬───────────────┐
│ Service          │ Address          │ Protocols    │  Status       │
├──────────────────┼──────────────────┼──────────────┼───────────────┤
│ HTTP Server      │ 0.0.0.0:{:<5}    │ HTTP         │ 🟢 Ready      │
│ GB28181 Session  │ 0.0.0.0:{:<5}    │ TCP, UDP     │ 🟢 Listening  │
└──────────────────┴──────────────────┴──────────────┴───────────────┘"#,
        http_port, rtp_port
    );
    f(msg);
}
