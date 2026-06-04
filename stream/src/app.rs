use crate::general::cfg::ServerConf;
use crate::io::{http, rtp_handler};
use crate::media;
use crate::state::register::Register;
use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::GlobalResult;
use base::log::info;
use base::logger;
use base::tokio::sync::mpsc;
use base::utils::rt::{GlobalRuntime, RuntimeType};
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
        let app = App {
            conf: ServerConf::init_by_conf(),
        };
        logger::Logger::init()?;
        let http_port = app.conf.http_port;
        let http_listener = http::listen_http_server(http_port)?;
        let rtp_port = app.conf.rtp_port;
        let tu = rtp_handler::listen_media_server(rtp_port)?;
        banner(Self::cli_basic().version, http_port, rtp_port, |msg| {
            info!("{msg}")
        });
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
        let (tx, rx) = mpsc::channel(100);
        Register::init()?;

        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        {
            let _enter = network_rt.rt_handle.enter();
            rtp_handler::run(tu, network_rt.cancel.clone())?;
        }
        network_rt
            .rt_handle
            .spawn(http::run(http_listener, tx, network_rt.cancel.clone()));

        let compute_rt = GlobalRuntime::register_default(RuntimeType::CommonCompute)?;
        compute_rt.rt_handle.spawn(media::handle_process(rx));

        GlobalRuntime::order_shutdown(
            &[RuntimeType::CommonNetwork, RuntimeType::CommonCompute],
            |msg| info!("{msg}"),
        );
        Ok(())
    }
}

fn banner<F: FnOnce(String)>(version: &str, http_port: u16, rtp_port: u16, f: F) {
    let msg = format!(
        r#"
            ___   __  __  __   __    _      ___    _____    ___    ___    ___    __  __
    o O O  / __| |  \/  | \ \ / /   (_)    / __|  |_   _|  | _ \  | __|  /   \  |  \/  |
   o      | (_ | | |\/| |  \ V /     _     \__ \    | |    |   /  | _|   | - |  | |\/| |
  oO__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   _|_|_   |_|_\  |___|  |_|_|  |_|__|_|
 [======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""T""|_|""R""|_|""E""|_|""A""|_|""M""|==]
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'
{:>30}: {}
┌──────────────────┬──────────────────┬──────────────┬──────────────┐
│ Service          │ Address          │ Protocols    │  Status      │
├──────────────────┼──────────────────┼──────────────┼──────────────┤
│ HTTP Server      │ 0.0.0.0:{:<5}    │ HTTP         │ 🟢 Ready     │
│ RTP Media Stream │ 0.0.0.0:{:<5}    │ TCP, UDP     │ 🟢 Listening │
└──────────────────┴──────────────────┴──────────────┴──────────────┘"#,
        "Version", version, http_port, rtp_port
    );
    f(msg);
}
