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
        Option<std::net::TcpListener>,
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
            Option<std::net::TcpListener>,
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
        let http_listener = if app_info.http.enabled {
            Some(app_info.http.listen_http_server()?)
        } else {
            None
        };
        let tu = app_info.session_conf.listen_gb_server()?;
        let grpc = crate::state::SessionGrpcConf::get();
        let grpc_listener = TcpListener::bind(grpc.addr).map_err(|error| {
            base::exception::GlobalError::new_sys_error(
                &format!("bind session grpc {} failed: {error}", grpc.addr),
                |_| {},
            )
        })?;
        banner(
            Self::cli_basic().version,
            app_info.http.port,
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
            Option<std::net::TcpListener>,
            (Option<std::net::TcpListener>, Option<UdpSocket>),
            TcpListener,
        ),
    ) -> GlobalResult<()> {
        let http = self.http;
        let node_id = self.session_conf.domain_id.clone();
        let http_enabled = http.enabled;
        let http_port = http.port;
        let grpc = crate::state::SessionGrpcConf::get();
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
            let mut node = SessionGuardNode::new(
                node_id,
                generate_instance_id(),
                http_enabled.then_some(u32::from(http_port)),
            );
            node.started_at_epoch_ms = started_at_epoch_ms;
            node.endpoints.push(Endpoint {
                name: "grpc".to_string(),
                scheme: grpc.scheme().to_string(),
                host: grpc.addr.ip().to_string(),
                port: u32::from(grpc.addr.port()),
                mode: EndpointMode::Single as i32,
                labels: HashMap::new(),
            });
            let control_identity = node.identity.clone();
            let control_node_id = control_identity.node_id.clone();
            let control_addr = grpc.addr;
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
            if let Some(http_listener) = http_listener {
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
            } else {
                warn!("HTTP service disabled by configuration");
                service_rt.cancel.cancelled().await;
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
    http_port: u16,
    sip_listen_addr: String,
    sip_advertised_addr: String,
    f: F,
) {
    let http_addr = format!("0.0.0.0:{http_port}");
    let msg = format!(
        r#"
======================================================================
              [GMV:SESSION-GB28181]   Version: {}
======================================================================
┌──────────────────┬──────────────────────┬──────────────┬──────────────┐
│ Service          │ Address              │ Protocols    │  Status      │
├──────────────────┼──────────────────────┼──────────────┼──────────────┤
│ Session HTTP     │ {:<20} │ HTTP         │ 🟢 Ready     │
│ SIP Listen       │ {:<20} │ TCP, UDP     │ 🟢 Listening │
│ SIP Advertised   │ {:<20} │ TCP, UDP     │ 🟢 Ready     │
└──────────────────┴──────────────────────┴──────────────┴──────────────┘"#,
        version, http_addr, sip_listen_addr, sip_advertised_addr
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
