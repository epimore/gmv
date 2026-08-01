use crate::general::cfg::{GuardConf, MediaListenerConf, MediaListenerMode, ServerConf};
use crate::io::media_endpoint::{MediaBootstrap, MediaEndpointManager};
use crate::io::{http, rtp_handler, talk::TalkManager};
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
use std::net::TcpListener;
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

pub struct StreamBootstrap {
    http_listener: TcpListener,
    grpc_listener: TcpListener,
    media: MediaBootstrap,
    media_conf: MediaListenerConf,
}

impl Daemon<StreamBootstrap> for App {
    fn cli_basic() -> CliBasic {
        default_cli_basic!()
    }

    fn init_privilege() -> GlobalResult<(Self, StreamBootstrap)>
    where
        Self: Sized,
    {
        let app = App {
            conf: ServerConf::init_by_conf(),
        };
        logger::Logger::init()?;
        let http_addr = app.conf.http.listen_addr;
        let media_conf = app.conf.media_listener_conf().map_err(|message| {
            base::exception::GlobalError::new_biz_error(
                base::err::BaseErrorCode::InvalidRequest.code(),
                &message,
                |msg| error!("{msg}"),
            )
        })?;
        let grpc_addr = app.conf.grpc.listen_addr;
        let http_listener = http::listen_http_server(http_addr)?;
        let grpc_listener = TcpListener::bind(grpc_addr).map_err(|error| {
            base::exception::GlobalError::new_sys_error(
                &format!("bind stream grpc {grpc_addr} failed: {error}"),
                |_| {},
            )
        })?;
        let media = match media_conf.mode {
            MediaListenerMode::Single => MediaBootstrap::Single {
                listener: rtp_handler::listen_media_server(
                    media_conf.bind_ip,
                    media_conf.single_port,
                )?,
            },
            MediaListenerMode::Multi => MediaBootstrap::Multi,
        };
        banner(Self::cli_basic().version, &app.conf, &media_conf, |msg| {
            info!("{msg}")
        });
        Ok((
            app,
            StreamBootstrap {
                http_listener,
                grpc_listener,
                media,
                media_conf,
            },
        ))
    }

    fn run_app(self, bootstrap: StreamBootstrap) -> GlobalResult<()> {
        let StreamBootstrap {
            http_listener,
            grpc_listener,
            media,
            media_conf,
        } = bootstrap;
        let node_name = self.conf.name.clone();
        let (http_public_tls, http_public_host, http_public_port) =
            self.conf.http.public_endpoint().map_err(|message| {
                base::exception::GlobalError::new_biz_error(
                    base::err::BaseErrorCode::InvalidRequest.code(),
                    &message,
                    |msg| error!("{msg}"),
                )
            })?;
        let grpc = self.conf.grpc.clone();
        let (grpc_advertised_tls, grpc_advertised_host, grpc_advertised_port) =
            grpc.advertised_endpoint().map_err(|message| {
                base::exception::GlobalError::new_biz_error(
                    base::err::BaseErrorCode::InvalidRequest.code(),
                    &message,
                    |msg| error!("{msg}"),
                )
            })?;
        let guard = GuardConf::init_by_conf();
        let started_at_epoch_ms = now_epoch_ms();
        let (tx, rx) = mpsc::channel(100);
        let network_rt = GlobalRuntime::register_default(RuntimeType::CommonNetwork)?;
        Register::init(&network_rt, self.conf.clone())?;
        {
            let _enter = network_rt.rt_handle.enter();
            let media_endpoints = MediaEndpointManager::new(network_rt.clone(), media_conf, media)?;
            MediaEndpointManager::install_global(media_endpoints.clone())?;
            MediaEndpointManager::spawn_expiry_task(media_endpoints.clone())?;
            TalkManager::init(network_rt.clone(), media_endpoints.clone())?;
            let receive_endpoint = media_endpoints.capability_endpoint();
            let mut node = StreamGuardNode::new(
                node_name,
                generate_instance_id(),
                http_public_host,
                guard.endpoint.clone(),
                u32::from(http_public_port),
                http_public_tls,
                receive_endpoint.port,
            );
            node.endpoints.retain(|endpoint| endpoint.name != "rtp");
            node.endpoints.push(receive_endpoint.clone());
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
            let control_cancel = network_rt.cancel.clone();
            let control_shutdown = control_cancel.clone();
            let control_rpc = StreamControlRpc::new(
                StreamControlAdapter::new(control_identity, receive_endpoint)
                    .with_media_endpoints(media_endpoints.clone())
                    .with_media_tx(tx.clone()),
            );
            let server_rpc = control_rpc.clone();
            network_rt.spawn("stream-control-rpc", async move {
                base::log::debug!(
                    "stream rpc service inbound: node_id={}, bind_addr={}, tls={}",
                    control_node_id,
                    control_addr,
                    grpc.tls.enabled
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
                                GlobalRuntime::request_shutdown_with_error();
                                return;
                            }
                        },
                    );
                }
                let incoming = match base_rpc::tcp_incoming_from_std(grpc_listener) {
                    Ok(incoming) => incoming,
                    Err(err) => {
                        error!("stream control RPC listener failed: {err}");
                        GlobalRuntime::request_shutdown_with_error();
                        return;
                    }
                };
                let mut server = match base_rpc::build_server(&server_config) {
                    Ok(server) => server,
                    Err(err) => {
                        error!("stream control RPC server build failed: {err}");
                        GlobalRuntime::request_shutdown_with_error();
                        return;
                    }
                };
                if let Err(err) = server
                    .add_service(StreamControlServer::new(server_rpc))
                    .serve_with_incoming_shutdown(incoming, async move {
                        control_shutdown.cancelled().await
                    })
                    .await
                {
                    error!("stream control RPC server stopped with error: {err}");
                    GlobalRuntime::request_shutdown_with_error();
                } else {
                    if !control_cancel.is_cancelled() {
                        error!("stream control RPC server stopped unexpectedly");
                        GlobalRuntime::request_shutdown_with_error();
                        return;
                    }
                    base::log::debug!(
                        "stream rpc service outbound: node_id={}, bind_addr={}",
                        control_node_id,
                        control_addr
                    );
                }
            })?;
            let mut reporter = NodeReporterConfig::new(
                node.guard_channel.clone(),
                node.register_request(NodeResourceSnapshot::default()),
            );
            reporter.resource_snapshot = Some(Arc::new(move || {
                let control_rpc = control_rpc.clone();
                Box::pin(async move { control_rpc.resource_snapshot().await })
            }));
            let metrics_media_endpoints = media_endpoints.clone();
            reporter.business_metrics = Arc::new(move || {
                let media_stats = metrics_media_endpoints.stats_snapshot();
                HashMap::from([
                    (
                        "receiving_streams".to_string(),
                        Register::active_stream_count().to_string(),
                    ),
                    (
                        "active_talk_sessions".to_string(),
                        TalkManager::active_session_count().to_string(),
                    ),
                    (
                        "media_ports_total".to_string(),
                        media_stats.total.to_string(),
                    ),
                    ("media_ports_free".to_string(), media_stats.free.to_string()),
                    ("media_ports_binding".to_string(), "0".to_string()),
                    (
                        "media_ports_listening".to_string(),
                        media_stats.listening.to_string(),
                    ),
                    (
                        "media_ports_confirmed".to_string(),
                        media_stats.confirmed.to_string(),
                    ),
                    (
                        "media_ports_releasing".to_string(),
                        media_stats.releasing.to_string(),
                    ),
                    (
                        "media_port_bind_failures".to_string(),
                        media_stats.bind_failures.to_string(),
                    ),
                    (
                        "media_port_exhaustions".to_string(),
                        media_stats.exhaustions.to_string(),
                    ),
                ])
            });
            init_guard_channel(node.guard_channel.clone());
            let event_sender = NodeReporter::spawn_managed_with_events(
                &network_rt,
                reporter,
                network_rt.cancel.clone(),
            )?;
            init_guard_event_sender(event_sender);
            let shutdown_cancel = network_rt.cancel.clone();
            network_rt.spawn("stream-media-endpoint-shutdown", async move {
                shutdown_cancel.cancelled().await;
                if let Err(error) = media_endpoints.shutdown().await {
                    error!("media endpoint shutdown failed: {error}");
                    GlobalRuntime::request_shutdown_with_error();
                }
            })?;
        }
        let http_cancel = network_rt.cancel.clone();
        let http_shutdown = http_cancel.clone();
        network_rt.spawn("stream-http", async move {
            let result = http::run(
                http_listener,
                self.conf.http.tls.enabled.then(|| http::HttpTlsConfig {
                    certificate_path: self.conf.http.tls.certificate_path.clone(),
                    private_key_path: self.conf.http.tls.private_key_path.clone(),
                }),
                tx,
                http_shutdown,
            )
            .await;
            match result {
                Ok(()) if http_cancel.is_cancelled() => {}
                Ok(()) => {
                    error!("stream HTTP service stopped unexpectedly");
                    GlobalRuntime::request_shutdown_with_error();
                }
                Err(err) => {
                    error!("stream HTTP service stopped with error: {err}");
                    GlobalRuntime::request_shutdown_with_error();
                }
            }
        })?;

        let compute_rt = GlobalRuntime::register_default(RuntimeType::CommonCompute)?;
        let dispatcher_rt = compute_rt.clone();
        compute_rt.spawn(
            "stream-media-dispatcher",
            media::handle_process(rx, dispatcher_rt),
        )?;

        let report = GlobalRuntime::order_shutdown(&[
            RuntimeType::CommonNetwork,
            RuntimeType::CommonCompute,
        ]);
        if !report.is_graceful() {
            return Err(base::exception::GlobalError::new_sys_error(
                "stream shutdown was incomplete",
                |_| {},
            ));
        }
        Ok(())
    }
}

fn banner<F: FnOnce(String)>(
    version: &str,
    server_conf: &ServerConf,
    media_conf: &MediaListenerConf,
    f: F,
) {
    let (rtp_listen, rtp_public, rtp_status) = match media_conf.mode {
        MediaListenerMode::Single => (
            format!("{}:{}", media_conf.bind_ip, media_conf.single_port),
            format!("{}:{}", media_conf.advertised_host, media_conf.single_port),
            "🟢 Listening",
        ),
        MediaListenerMode::Multi => (
            format!(
                "{}:{}-{}",
                media_conf.bind_ip, media_conf.port_range.start, media_conf.port_range.end
            ),
            format!(
                "{}:{}-{}",
                media_conf.advertised_host, media_conf.port_range.start, media_conf.port_range.end
            ),
            "🟢 Dynamic",
        ),
    };
    let msg = format!(
        r#"
======================================================================
                    [GMV:STREAM]   Version: {}
======================================================================
HTTP listen       : {}
HTTP public       : {}
gRPC listen       : {}
gRPC advertised   : {}
RTP listen        : {}
RTP advertised    : {}
RTP status        : {}"#,
        version,
        server_conf.http.listen_addr,
        server_conf.http.public_url,
        server_conf.grpc.listen_addr,
        server_conf.grpc.advertised_url,
        rtp_listen,
        rtp_public,
        rtp_status
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
