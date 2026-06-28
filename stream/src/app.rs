use crate::general::cfg::{GrpcConf, GuardConf, ServerConf};
use crate::io::{http, rtp_handler};
use crate::media;
use crate::state::register::Register;
use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::GlobalResult;
use base::log::{error, info};
use base::logger;
use base::tokio::sync::mpsc;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use std::collections::HashMap;
use std::net::{TcpListener, UdpSocket};
use std::sync::Arc;

use crate::guard_integration::{
    StreamControlAdapter, StreamControlRpc, StreamGuardNode, init_guard_channel,
    init_guard_event_sender,
};
use gmv_nodec::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::common::v1::{Endpoint, EndpointMode};
use gmv_protocol::guard::v1::NodeResourceSnapshot;
use gmv_protocol::stream::v1::stream_control_server::StreamControlServer;

pub struct App {
    conf: ServerConf,
}

impl
    Daemon<(
        std::net::TcpListener,
        (Option<std::net::TcpListener>, Option<UdpSocket>),
        TcpListener,
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
            TcpListener,
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
        let grpc = GrpcConf::init_by_conf();
        let grpc_listener = TcpListener::bind(grpc.addr).map_err(|error| {
            base::exception::GlobalError::new_sys_error(
                &format!("bind stream grpc {} failed: {error}", grpc.addr),
                |_| {},
            )
        })?;
        banner(Self::cli_basic().version, http_port, rtp_port, |msg| {
            info!("{msg}")
        });
        Ok((app, (http_listener, tu, grpc_listener)))
    }

    fn run_app(
        self,
        t: (
            std::net::TcpListener,
            (Option<std::net::TcpListener>, Option<UdpSocket>),
            TcpListener,
        ),
    ) -> GlobalResult<()> {
        let (http_listener, tu, grpc_listener) = t;
        let node_name = self.conf.name.clone();
        let http_port = self.conf.http_port;
        let rtp_port = self.conf.rtp_port;
        let host = self.conf.host.clone();
        let grpc = GrpcConf::init_by_conf();
        let guard = GuardConf::init_by_conf();
        let started_at_epoch_ms = now_epoch_ms();
        let (tx, rx) = mpsc::channel(100);
        Register::init()?;

        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        {
            let _enter = network_rt.rt_handle.enter();
            rtp_handler::run(tu, network_rt.cancel.clone())?;
            let mut node = StreamGuardNode::new(
                node_name,
                generate_instance_id(),
                host.clone(),
                guard.endpoint.clone(),
                u32::from(http_port),
                self.conf.http.tls.enabled,
                u32::from(rtp_port),
            );
            node.started_at_epoch_ms = started_at_epoch_ms;
            node.endpoints.push(Endpoint {
                name: "grpc".to_string(),
                scheme: base_rpc::rpc_scheme(grpc.tls.enabled).to_string(),
                host: grpc.addr.ip().to_string(),
                port: u32::from(grpc.addr.port()),
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            });
            let control_identity = node.identity.clone();
            let receive_endpoint = Endpoint {
                name: "rtp".to_string(),
                scheme: "rtp".to_string(),
                host: host.clone(),
                port: u32::from(rtp_port),
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            };
            let control_cancel = network_rt.cancel.clone();
            let control_media_tx = tx.clone();
            base::tokio::spawn(async move {
                let rpc = StreamControlRpc::new(
                    StreamControlAdapter::new(control_identity, receive_endpoint)
                        .with_media_tx(control_media_tx),
                );
                let mut server_config = base_rpc::RpcServerConfig::default();
                if grpc.tls.enabled {
                    server_config.tls = Some(
                        match base_rpc::load_server_tls_from_files(&base_rpc::TlsFileConfig {
                            certificate_path: Some(grpc.tls.certificate_path.clone()),
                            private_key_path: Some(grpc.tls.private_key_path.clone()),
                            ..base_rpc::TlsFileConfig::default()
                        }) {
                            Ok(tls) => tls,
                            Err(err) => {
                                error!("stream control RPC TLS config failed: {err}");
                                return;
                            }
                        },
                    );
                }
                let incoming = match base_rpc::tcp_incoming_from_std(grpc_listener) {
                    Ok(incoming) => incoming,
                    Err(err) => {
                        error!("stream control RPC listener failed: {err}");
                        return;
                    }
                };
                let mut server = match base_rpc::build_server(&server_config) {
                    Ok(server) => server,
                    Err(err) => {
                        error!("stream control RPC server build failed: {err}");
                        return;
                    }
                };
                if let Err(err) = server
                    .add_service(StreamControlServer::new(rpc))
                    .serve_with_incoming_shutdown(incoming, async move {
                        control_cancel.cancelled().await
                    })
                    .await
                {
                    error!("stream control RPC server stopped with error: {err}");
                }
            });
            let mut reporter = NodeReporterConfig::new(
                node.guard_channel.clone(),
                node.register_request(NodeResourceSnapshot::default()),
            );
            reporter.business_metrics = Arc::new(|| {
                HashMap::from([(
                    "receiving_streams".to_string(),
                    Register::active_stream_count().to_string(),
                )])
            });
            init_guard_channel(node.guard_channel.clone());
            let (_reporter, event_sender) =
                NodeReporter::spawn_with_events(reporter, network_rt.cancel.clone());
            init_guard_event_sender(event_sender);
        }
        network_rt.rt_handle.spawn(http::run(
            http_listener,
            self.conf.http.tls.enabled.then(|| http::HttpTlsConfig {
                certificate_path: self.conf.http.tls.certificate_path.clone(),
                private_key_path: self.conf.http.tls.private_key_path.clone(),
            }),
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

fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            duration.as_millis().min(i64::MAX as u128) as i64
        })
}
