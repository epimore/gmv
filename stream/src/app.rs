use crate::general::cfg::ServerConf;
use crate::io::{http, rtp_handler};
use crate::media;
use crate::state::cache;
use base::daemon::Daemon;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, info};
use base::tokio::sync::mpsc;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use base::{logger, tokio};
use std::net::UdpSocket;

pub struct App {
    conf: ServerConf,
}

impl
    Daemon<(
        std::net::TcpListener,
        (Option<std::net::TcpListener>, Option<UdpSocket>),
    )> for App
{
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
        let app = App {
            conf: cache::get_server_conf().clone(),
        };
        logger::Logger::init()?;
        let http_port = *app.conf.get_http_port();
        let http_listener = http::listen_http_server(http_port)?;
        let rtp_port = *app.conf.get_rtp_port();
        let tu = rtp_handler::listen_media_server(rtp_port)?;
        banner(http_port, rtp_port, |msg| info!("{msg}"));
        Ok((app, (http_listener, tu)))
    }

    fn run_app(
        self,
        t: (
            std::net::TcpListener,
            (Option<std::net::TcpListener>, Option<UdpSocket>),
        ),
    ) -> GlobalResult<()> {
        let (http_listener, tu) = t;
        let conf = self.conf;
        let node_name = conf.get_name().clone();
        let (tx, rx) = mpsc::channel(100);

        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        network_rt
            .rt_handle
            .spawn(rtp_handler::run(tu, network_rt.cancel.clone()));
        network_rt.rt_handle.spawn(http::run(
            node_name,
            http_listener,
            tx,
            network_rt.cancel.clone(),
        ));

        let compute_rt = GlobalRuntime::register_default(RuntimeType::CommonCompute)?;
        compute_rt.rt_handle.spawn(media::handle_process(rx));

        GlobalRuntime::order_shutdown(
            &[RuntimeType::CommonNetwork, RuntimeType::CommonCompute],
            |msg| info!("{msg}"),
        );
        Ok(())
    }
}

fn banner<F: FnOnce(String)>(http_port: u16, rtp_port: u16, f: F) {
    let msg = format!(
        r#"
            ___   __  __  __   __    _      ___    _____    ___    ___    ___    __  __
    o O O  / __| |  \/  | \ \ / /   (_)    / __|  |_   _|  | _ \  | __|  /   \  |  \/  |
   o      | (_ | | |\/| |  \ V /     _     \__ \    | |    |   /  | _|   | - |  | |\/| |
  oO__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   _|_|_   |_|_\  |___|  |_|_|  |_|__|_|
 [======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""T""|_|""R""|_|""E""|_|""A""|_|""M""|==]
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'

┌──────────────────┬──────────────────┬──────────────┬──────────────┐
│ Service          │ Address          │ Protocols    │  Status      │
├──────────────────┼──────────────────┼──────────────┼──────────────┤
│ HTTP Server      │ 0.0.0.0:{:<5}    │ HTTP         │ 🟢 Ready     │
│ RTP Media Stream │ 0.0.0.0:{:<5}    │ TCP, UDP     │ 🟢 Listening │
└──────────────────┴──────────────────┴──────────────┴──────────────┘"#,
        http_port, rtp_port
    );
    f(msg);
}
