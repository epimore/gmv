use crate::io::talk::TalkManager;
use axum::Router;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Path, Query};
use axum::response::{IntoResponse, Response};
use base::log::{debug, info, warn};
use futures_util::StreamExt;
use gmv_domain::info::obj::TALK_INPUT_PATH;
use std::collections::HashMap;

pub fn routes() -> Router {
    Router::new().route(TALK_INPUT_PATH, axum::routing::get(talk_input_ws))
}

async fn talk_input_ws(
    Path(talk_id): Path<String>,
    Query(mut query): Query<HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(token) = query.remove("gmv-token") else {
        return super::res_401();
    };
    if !TalkManager::check_token(&talk_id, &token) {
        return super::res_401();
    }
    ws.on_upgrade(move |socket| handle_talk_socket(talk_id, socket))
        .into_response()
}

async fn handle_talk_socket(talk_id: String, mut socket: WebSocket) {
    info!("talk websocket opened: talk_id={}", talk_id);
    while let Some(msg) = socket.next().await {
        match msg {
            Ok(Message::Binary(frame)) => {
                if let Err(err) = TalkManager::push_frame(&talk_id, frame.to_vec()) {
                    warn!(
                        "talk input frame dropped: talk_id={}, err={:?}",
                        talk_id, err
                    );
                }
            }
            Ok(Message::Text(text)) => {
                debug!(
                    "talk input metadata ignored: talk_id={}, text={}",
                    talk_id, text
                );
            }
            Ok(Message::Close(_)) => {
                break;
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) => {}
            Err(err) => {
                warn!("talk websocket error: talk_id={}, err={}", talk_id, err);
                break;
            }
        }
    }
    info!("talk websocket closed: talk_id={}", talk_id);
}
