use crate::gb::SessionConf;
use crate::http::Http;
use crate::storage::db::{self, SessionDatabaseBackend};
use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::GlobalResult;
use base::log::{error, info, warn};
use base::logger;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::Arc;

use crate::guard_integration::{
    SessionControlAdapter, SessionControlRpc, SessionGuardNode, SessionHookRpc,
    init_guard_event_sender,
};
use crate::register::core::Register;
use gmv_nodec::{NodeReporter, NodeReporterConfig, generate_instance_id};
use gmv_protocol::common::v1::{Endpoint, EndpointMode};
use gmv_protocol::guard::v1::NodeResourceSnapshot;
use gmv_protocol::session::v1::session_control_server::SessionControlServer;
use gmv_protocol::session::v1::session_hook_server::SessionHookServer;

#[derive(Debug)]
pub struct AppInfo {
    session_conf: SessionConf,
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
            session_conf: SessionConf::get_session_by_conf(),
            http: Http::get_http_by_conf(),
        };
        logger::Logger::init()?;
        match db::backend() {
            SessionDatabaseBackend::Mysql => info!("session database backend: mysql"),
            SessionDatabaseBackend::Sqlite => {
                let _ = db::sqlite_pool();
                info!("session database backend: sqlite");
            }
        }
        let http_listener = app_info.http.listen_http_server()?;
        let tu = app_info.session_conf.listen_gb_server()?;
        banner(
            Self::cli_basic().version,
            app_info.http.port,
            app_info.session_conf.wan_port,
            |msg| info!("{msg}"),
        );
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
        let node_id = self.session_conf.domain_id.clone();
        let http_port = http.port;
        let grpc = crate::state::SessionGrpcConf::get();
        let started_at_epoch_ms = now_epoch_ms();
        let (http_listener, tu) = t;
        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        let service_cancel = network_rt.cancel.clone();
        let service_task = network_rt.rt_handle.spawn(async move {
            if let Err(err) = SessionConf::run(tu, network_rt.cancel.clone()).await {
                error!("GB28181 session initialization failed: {err}");
                warn!("session network task is cancelling after GB28181 initialization failure");
                network_rt.cancel.cancel();
                return;
            }
            let mut node =
                SessionGuardNode::new(node_id, generate_instance_id(), u32::from(http_port));
            node.started_at_epoch_ms = started_at_epoch_ms;
            node.endpoints.push(Endpoint {
                name: "grpc".to_string(),
                scheme: "grpc".to_string(),
                host: grpc.addr.ip().to_string(),
                port: u32::from(grpc.addr.port()),
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            });
            let control_identity = node.identity.clone();
            let control_cancel = network_rt.cancel.clone();
            base::tokio::spawn(async move {
                let address = grpc.addr;
                let rpc = SessionControlRpc::new(SessionControlAdapter::new(control_identity));
                if let Err(err) = tonic::transport::Server::builder()
                    .add_service(SessionControlServer::new(rpc))
                    .add_service(SessionHookServer::new(SessionHookRpc))
                    .serve_with_shutdown(address, async move { control_cancel.cancelled().await })
                    .await
                {
                    error!("session control RPC server stopped with error: {err}");
                }
            });
            let mut reporter = NodeReporterConfig::new(
                node.guard_channel.clone(),
                node.register_request(NodeResourceSnapshot::default()),
            );
            reporter.business_metrics = Arc::new(|| {
                HashMap::from([(
                    "active_devices".to_string(),
                    Register::active_device_count().to_string(),
                )])
            });
            let (_reporter, event_sender) =
                NodeReporter::spawn_with_events(reporter, network_rt.cancel.clone());
            init_guard_event_sender(event_sender);
            match http.run(http_listener, network_rt.cancel.clone()).await {
                Ok(()) => warn!(
                    "HTTP service returned; cancellation_requested={}",
                    network_rt.cancel.is_cancelled()
                ),
                Err(err) => {
                    error!("HTTP service stopped with error: {err}");
                    warn!("session network task is cancelling after HTTP service failure");
                    network_rt.cancel.cancel();
                }
            }
            warn!("session network task exited");
        });
        network_rt.rt_handle.spawn(async move {
            match service_task.await {
                Ok(()) => warn!(
                    "session network task completed; cancellation_requested={}",
                    service_cancel.is_cancelled()
                ),
                Err(err) => warn!(
                    "session network task terminated unexpectedly: cancelled={}, panic={}, err={err}",
                    err.is_cancelled(),
                    err.is_panic()
                ),
            }
        });
        GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork], |msg| info!("{msg}"));
        Ok(())
    }
}

fn banner<F: FnOnce(String)>(version: &str, http_port: u16, rtp_port: u16, f: F) {
    let msg = format!(
        r#"
            ___   __  __  __   __    _      ___     ___     ___     ___     ___     ___     _  _
    o O O  / __| |  \/  | \ \ / /   (_)    / __|   | __|   / __|   / __|   |_ _|   / _ \   | \| |
   o      | (_ | | |\/| |  \ V /     _     \__ \   | _|    \__ \   \__ \    | |   | (_) |  | .` |
  o0__[O]  \___| |_|__|_|  _\_/_   _(_)_   |___/   |___|   |___/   |___/   |___|   \___/   |_|\_|
 [======|_|""G""|_|""M""|_|""V""|_|"":""|_|""S""|_|""E""|_|""S""|_|""S""|_|""I""|_|""O""|_|""N""|==]
./0--000'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'"`-0-0-'
{:>30}: {}
┌──────────────────┬──────────────────┬──────────────┬───────────────┐
│ Service          │ Address          │ Protocols    │  Status       │
├──────────────────┼──────────────────┼──────────────┼───────────────┤
│ HTTP Server      │ 0.0.0.0:{:<5}    │ HTTP         │ 🟢 Ready      │
│ GB28181 Session  │ 0.0.0.0:{:<5}    │ TCP, UDP     │ 🟢 Listening  │
└──────────────────┴──────────────────┴──────────────┴───────────────┘"#,
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
