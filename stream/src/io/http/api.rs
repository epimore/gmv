use crate::io::broadcast::BroadcastManager;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use base::log::{debug, info, warn};
use futures_util::StreamExt;
use gmv_domain::info::obj::BROADCAST_INPUT_PATH;
use std::collections::HashMap;

pub fn routes() -> Router {
    Router::new().route(BROADCAST_INPUT_PATH, axum::routing::get(broadcast_input_ws))
}

async fn broadcast_input_ws(
    Path(broadcast_id): Path<String>,
    Query(mut query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = query.remove("gmv-token") else {
        return super::res_401();
    };
    if !BroadcastManager::check_token(&broadcast_id, &token) {
        return super::res_401();
    }
    ws.on_upgrade(move |socket| handle_broadcast_socket(broadcast_id, socket))
        .into_response()
}

async fn handle_broadcast_socket(broadcast_id: String, mut socket: WebSocket) {
    info!("broadcast websocket opened: broadcast_id={}", broadcast_id);
    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(frame)) => {
                if let Err(err) = BroadcastManager::push_frame(&broadcast_id, frame.to_vec()) {
                    warn!(
                        "broadcast input frame dropped: broadcast_id={}, err={:?}",
                        broadcast_id, err
                    );
                }
            }
            Ok(Message::Text(text)) => {
                debug!(
                    "broadcast input metadata ignored: broadcast_id={}, text={}",
                    broadcast_id, text
                );
            }
            Ok(Message::Close(_)) => {
                break;
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                warn!(
                    "broadcast websocket error: broadcast_id={}, err={}",
                    broadcast_id, err
                );
                break;
            }
        }
    }
    info!("broadcast websocket closed: broadcast_id={}", broadcast_id);
}
