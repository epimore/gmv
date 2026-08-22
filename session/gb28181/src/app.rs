use crate::gb::SessionConf;
use crate::http::Http;
use crate::storage::db::{self, SessionDatabaseBackend};
use base::cfg_lib::{CliBasic, default_cli_basic};
use base::daemon::Daemon;
use base::exception::GlobalResult;
use base::log::{error, info};
use base::logger;
use base::utils::rt::{GlobalRuntime, RuntimeType};
use std::collections::HashMap;
use std::net::{TcpListener, UdpSocket};
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
        TcpListener,
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
            TcpListener,
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
            #[cfg(feature = "db-mysql")]
            SessionDatabaseBackend::Mysql => info!("session database backend: mysql"),
            #[cfg(not(feature = "db-mysql"))]
            SessionDatabaseBackend::Mysql => {
                return Err(db::backend_not_enabled_global(
                    SessionDatabaseBackend::Mysql,
                ));
            }
            #[cfg(feature = "db-sqlite")]
            SessionDatabaseBackend::Sqlite => info!("session database backend: sqlite"),
            #[cfg(not(feature = "db-sqlite"))]
            SessionDatabaseBackend::Sqlite => {
                return Err(db::backend_not_enabled_global(
                    SessionDatabaseBackend::Sqlite,
                ));
            }
        }
        let http_listener = app_info.http.listen_http_server()?;
        let tu = app_info.session_conf.listen_gb_server()?;
        let grpc = crate::state::SessionGrpcConf::get();
        let grpc_listener = TcpListener::bind(grpc.listen_addr).map_err(|error| {
            base::exception::GlobalError::new_sys_error(
                &format!("bind session grpc {} failed: {error}", grpc.listen_addr),
                |_| {},
            )
        })?;
        banner(
            Self::cli_basic().version,
            &app_info.http,
            &grpc,
            format!(
                "{}:{}",
                app_info.session_conf.lan_ip, app_info.session_conf.wan_port
            ),
            format!(
                "{}:{}",
                app_info.session_conf.wan_ip, app_info.session_conf.wan_port
            ),
            |msg| info!("{msg}"),
        );
        Ok((app_info, (http_listener, tu, grpc_listener)))
    }

    fn run_app(
        self,
        t: (
            std::net::TcpListener,
            (Option<std::net::TcpListener>, Option<UdpSocket>),
            TcpListener,
        ),
    ) -> GlobalResult<()> {
        let http = self.http;
        let node_id = self.session_conf.domain_id.clone();
        let http_endpoint = http
            .public_endpoint()
            .expect("validated session HTTP public URL");
        let grpc = crate::state::SessionGrpcConf::get();
        let (grpc_advertised_tls, grpc_advertised_host, grpc_advertised_port) = grpc
            .advertised_endpoint()
            .expect("validated session gRPC advertised URL");
        let started_at_epoch_ms = now_epoch_ms();
        let (http_listener, tu, grpc_listener) = t;
        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        let service_rt = network_rt.clone();
        network_rt.spawn("session-service", async move {
            if let Err(err) = SessionConf::run(tu, &service_rt).await {
                error!("GB28181 session initialization failed: {err}");
                GlobalRuntime::request_shutdown_with_error();
                return;
            }
            let mut node = SessionGuardNode::new(node_id, generate_instance_id(), http_endpoint);
            node.started_at_epoch_ms = started_at_epoch_ms;
            node.endpoints.push(Endpoint {
                name: "grpc".to_string(),
                scheme: base_rpc::rpc_scheme(grpc_advertised_tls).to_string(),
                host: grpc_advertised_host,
                port: u32::from(grpc_advertised_port),
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            });
            let control_identity = node.identity.clone();
            let control_node_id = control_identity.node_id.clone();
            let control_addr = grpc.listen_addr;
            let control_cancel = service_rt.cancel.clone();
            let control_shutdown = control_cancel.clone();
            if let Err(err) = service_rt.spawn("session-control-rpc", async move {
                base::log::debug!(
                    "session rpc service inbound: node_id={}, bind_addr={}, tls={}",
                    control_node_id,
                    control_addr,
                    grpc.tls.enabled
                );
                let rpc = SessionControlRpc::new(SessionControlAdapter::new(control_identity));
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
                                error!("session control RPC TLS config failed: {err}");
                                GlobalRuntime::request_shutdown_with_error();
                                return;
                            }
                        },
                    );
                }
                let incoming = match base_rpc::tcp_incoming_from_std(grpc_listener) {
                    Ok(incoming) => incoming,
                    Err(err) => {
                        error!("session control RPC listener failed: {err}");
                        GlobalRuntime::request_shutdown_with_error();
                        return;
                    }
                };
                let mut server = match base_rpc::build_server(&server_config) {
                    Ok(server) => server,
                    Err(err) => {
                        error!("session control RPC server build failed: {err}");
                        GlobalRuntime::request_shutdown_with_error();
                        return;
                    }
                };
                if let Err(err) = server
                    .add_service(SessionControlServer::new(rpc))
                    .add_service(SessionHookServer::new(SessionHookRpc))
                    .serve_with_incoming_shutdown(incoming, async move {
                        control_shutdown.cancelled().await
                    })
                    .await
                {
                    error!("session control RPC server stopped with error: {err}");
                    GlobalRuntime::request_shutdown_with_error();
                } else {
                    if !control_cancel.is_cancelled() {
                        error!("session control RPC server stopped unexpectedly");
                        GlobalRuntime::request_shutdown_with_error();
                        return;
                    }
                    base::log::debug!(
                        "session rpc service outbound: node_id={}, bind_addr={}",
                        control_node_id,
                        control_addr
                    );
                }
            }) {
                error!("spawn session control RPC task failed: {err}");
                GlobalRuntime::request_shutdown_with_error();
                return;
            }
            let mut reporter = NodeReporterConfig::new(
                node.guard_channel.clone(),
                node.register_request(NodeResourceSnapshot::default()),
            );
            reporter.business_metrics = Arc::new(|| {
                HashMap::from([
                    (
                        "active_devices".to_string(),
                        Register::active_device_count().to_string(),
                    ),
                    (
                        "catalog_subscription_degraded_devices".to_string(),
                        crate::gb::sip::subscription::degraded_catalog_subscription_count()
                            .to_string(),
                    ),
                    (
                        "dialog_runtime_conflicts".to_string(),
                        crate::service::dialog_recovery::runtime_dialog_conflict_count()
                            .to_string(),
                    ),
                ])
            });
            let event_sender = match NodeReporter::spawn_managed_with_events(
                &service_rt,
                reporter,
                service_rt.cancel.clone(),
            ) {
                Ok(started) => started,
                Err(err) => {
                    error!("spawn session node reporter failed: {err}");
                    GlobalRuntime::request_shutdown_with_error();
                    return;
                }
            };
            init_guard_event_sender(event_sender);
            match http.run(http_listener, service_rt.cancel.clone()).await {
                Ok(()) if service_rt.cancel.is_cancelled() => {
                    base::log::debug!("HTTP service returned after cancellation")
                }
                Ok(()) => {
                    error!("HTTP service stopped unexpectedly");
                    GlobalRuntime::request_shutdown_with_error();
                }
                Err(err) => {
                    error!("HTTP service stopped with error: {err}");
                    GlobalRuntime::request_shutdown_with_error();
                }
            }
            base::log::debug!("session network task exited");
        })?;
        let report = GlobalRuntime::order_shutdown(&[RuntimeType::CommonNetwork]);
        if !report.is_graceful() {
            return Err(base::exception::GlobalError::new_sys_error(
                "session shutdown was incomplete",
                |_| {},
            ));
        }
        Ok(())
    }
}

fn banner<F: FnOnce(String)>(
    version: &str,
    http: &Http,
    grpc: &crate::state::SessionGrpcConf,
    sip_listen_addr: String,
    sip_advertised_addr: String,
    f: F,
) {
    let http_listen_protocol = if http.tls.enabled { "HTTPS" } else { "HTTP" };
    let http_public_protocol = if http
        .public_endpoint()
        .expect("validated session HTTP public URL")
        .0
    {
        "HTTPS"
    } else {
        "HTTP"
    };
    let grpc_listen_protocol = if grpc.tls.enabled { "gRPC/TLS" } else { "gRPC" };
    let grpc_advertised_protocol = if grpc
        .advertised_endpoint()
        .expect("validated session gRPC advertised URL")
        .0
    {
        "gRPC/TLS"
    } else {
        "gRPC"
    };
    let http_listen_addr = http.listen_addr.to_string();
    let http_public_url = &http.public_url;
    let grpc_listen_addr = grpc.listen_addr.to_string();
    let grpc_advertised_url = &grpc.advertised_url;
    let address_width = [
        http_listen_addr.len(),
        http_public_url.len(),
        grpc_listen_addr.len(),
        grpc_advertised_url.len(),
        sip_listen_addr.len(),
        sip_advertised_addr.len(),
    ]
    .into_iter()
    .max()
    .unwrap_or(0)
    .max(32);
    let address_border = "─".repeat(address_width + 2);
    let address_header = "Address";
    let banner_width = address_width + 53;
    let separator = "=".repeat(banner_width);
    let title = format!("[GMV:SESSION-GB28181]   Version: {version}");
    let msg = format!(
        r#"
{separator}
{title:^banner_width$}
{separator}
┌──────────────────┬{address_border}┬──────────────┬──────────────┐
│ Service          │ {address_header:<address_width$} │ Protocols    │  Status      │
├──────────────────┼{address_border}┼──────────────┼──────────────┤
│ Session HTTP     │ {http_listen_addr:<address_width$} │ {http_listen_protocol:<12} │ 🟢 Ready     │
│ HTTP Public      │ {http_public_url:<address_width$} │ {http_public_protocol:<12} │ 🟢 Ready     │
│ Session RPC      │ {grpc_listen_addr:<address_width$} │ {grpc_listen_protocol:<12} │ 🟢 Listening │
│ RPC Advertised   │ {grpc_advertised_url:<address_width$} │ {grpc_advertised_protocol:<12} │ 🟢 Ready     │
│ SIP Listen       │ {sip_listen_addr:<address_width$} │ TCP, UDP     │ 🟢 Listening │
│ SIP Advertised   │ {sip_advertised_addr:<address_width$} │ TCP, UDP     │ 🟢 Ready     │
└──────────────────┴{address_border}┴──────────────┴──────────────┘"#
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
