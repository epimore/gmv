use axum::Router;
use common::exception::{GlobalResult, TransError};
use common::log::{error, info};
use common::tokio::net::TcpListener;
use common::tokio::sync::mpsc::Sender;

mod flv;
mod hls;
mod dash;
mod api;
pub mod call;

pub fn listen_http_server(port: u16) -> GlobalResult<std::net::TcpListener> {
    let listener = std::net::TcpListener::bind(format!("0.0.0.0:{}", port)).hand_log(|msg| error!("{msg}"))?;
    info!("Listen to http web addr = 0.0.0.0:{} ...", port);
    Ok(listener)
}

pub async fn run(node: &String, std_http_listener: std::net::TcpListener, tx: Sender<u32>) -> GlobalResult<()> {
    std_http_listener.set_nonblocking(true).hand_log(|msg| error!("{msg}"))?;
    let listener = TcpListener::from_std(std_http_listener).hand_log(|msg| error!("{msg}"))?;
    let app = Router::new()
        .merge(flv::routes(node))
        .merge(hls::routes())
        // .merge(dash::routes())
        .merge(api::routes(tx.clone()));

    axum::serve(listener, app)
        .await
        .hand_log(|msg| error!("{msg}"))?;
    Ok(())
}

/// 启动流式响应，包括连接断开监听、构建响应
pub async fn handle_streaming_request<F, S>(
    req: Request<Body>,
    make_stream: F,
) -> Response<Body>
where
    F: FnOnce(StreamContext) -> S + Send + 'static,
    S: Stream<Item=Result<bytes::Bytes, std::io::Error>> + Send + 'static,
{
    let (parts, _body) = req.into_parts();
    let headers = parts.headers.clone();
    let upgrade_fut = parts
        .extensions
        .get::<hyper::upgrade::OnUpgrade>()
        .cloned();

    let ctx = StreamContext::new(headers.clone());
    let disconnect_notify = ctx.client_disconnected.clone();

    // 启动后台任务检测断开
    if let Some(on_upgrade) = upgrade_fut {
        tokio::spawn(async move {
            if let Ok(upgraded) = on_upgrade.await {
                // 阻塞直到 socket 完全断开
                let _ = tokio::io::copy(&mut &upgraded, &mut tokio::io::sink()).await;
                disconnect_notify.notify_one();
            }
        });
    }

    // 构造媒体输出流
    let stream = make_stream(ctx);

    // 转为 Axum 可响应的 HTTP Body
    Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "video/x-flv") // 可替换为不同类型
        .body(Body::from_stream(stream))
        .unwrap_or_else(|_| Response::builder().status(500).body(Body::empty()).unwrap())
}
