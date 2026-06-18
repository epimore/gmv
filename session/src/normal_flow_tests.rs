use std::collections::HashMap;
use std::io::Cursor;
use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::{Body, to_bytes};
use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use base::cfg_lib::conf::init_cfg;
use base::serde::Serialize;
use base::serde_json::{Value, json};
use base::tokio::net::{TcpListener, UdpSocket};
use base::tokio::time::{sleep, timeout};
use base::tokio_util::sync::CancellationToken;
use gmv_pjsip::gb28181::xml::extract_xml_value_lossy;
use gmv_pjsip::{SipRuntimeSockets, SipTransportProtocol};
use image::{DynamicImage, ImageFormat};
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{
    BaseStreamInfo, RegisterStreamInfo, RtpInfo, StreamInfoQo, StreamKey, StreamRecordInfo,
    TalkAnswerReq, TalkCloseReq, TalkOpenReq, TalkOpenResp,
};
use shared::info::output::OutputEnum;
use shared::info::res::Resp;
use tower::ServiceExt;

use crate::gb::SessionConf;
use crate::gb::sip::auth;
use crate::gb::sip::command;
use crate::gb::sip::native_runtime::{
    NativeSipRuntimeHandle, NativeSipRuntimeService, RUNTIME_TEST_LOCK,
};
use crate::http;
use crate::register::core::Register;
use crate::service::hook_serv;
use crate::state;
use crate::storage::db_task;
use crate::storage::entity::{GmvOauth, enable_test_storage, test_file_id_by_biz_id};
use crate::utils::edge_token;

const DEVICE_ID: &str = "34020000001110000009";
const CHANNEL_ID: &str = "34020000001320000102";
const PLAYBACK_CHANNEL_ID: &str = "34020000001320000103";
const PLATFORM_ID: &str = "34020000002000000001";

#[derive(Default)]
struct MediaState {
    streams: Mutex<HashMap<u32, String>>,
}

async fn listen_media(
    State(state): State<Arc<MediaState>>,
    Json(config): Json<MediaConfig>,
) -> Json<Resp<()>> {
    state
        .streams
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(config.ssrc, config.stream_id);
    Json(Resp::build_success())
}

async fn sdp_media(
    State(state): State<Arc<MediaState>>,
    Json(media): Json<MediaMap>,
) -> Json<Resp<()>> {
    let stream_id = state
        .streams
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&media.ssrc)
        .cloned()
        .expect("stream initialized before SDP");
    base::tokio::spawn(async move {
        sleep(Duration::from_millis(20)).await;
        hook_serv::stream_register(RegisterStreamInfo {
            base_stream_info: BaseStreamInfo {
                rtp_info: RtpInfo {
                    ssrc: media.ssrc,
                    origin_trans: None,
                    server_name: "s1".into(),
                    proxy_addr: "http://127.0.0.1:18570".into(),
                },
                stream_id,
                in_time: 1,
            },
            code: 200,
            msg: None,
        })
        .await;
    });
    Json(Resp::build_success())
}

async fn stream_online(Json(_key): Json<StreamKey>) -> Json<Resp<bool>> {
    Json(Resp::build_success_data(true))
}

async fn record_info(Json(_info): Json<StreamInfoQo>) -> Json<Resp<StreamRecordInfo>> {
    Json(Resp::build_success_data(StreamRecordInfo {
        path_file_name: None,
        file_size: 4096,
        timestamp: 30,
        state: 1,
    }))
}

async fn close_output(Json(_info): Json<StreamInfoQo>) -> Json<Resp<()>> {
    Json(Resp::build_success())
}

async fn talk_open(Json(request): Json<TalkOpenReq>) -> Json<Resp<TalkOpenResp>> {
    Json(Resp::build_success_data(TalkOpenResp {
        talk_id: request.talk_id.clone(),
        input_url: format!("http://127.0.0.1:18570/talk/input/{}", request.talk_id),
        rtp_port: 18_572,
        codec: request.codec,
        sample_rate: request.sample_rate,
        channel_count: request.channel_count,
        payload_type: request.payload_type,
        frame_duration_ms: request.frame_duration_ms,
    }))
}

async fn talk_answer(Json(_request): Json<TalkAnswerReq>) -> Json<Resp<()>> {
    Json(Resp::build_success())
}

async fn talk_close(Json(_request): Json<TalkCloseReq>) -> Json<Resp<()>> {
    Json(Resp::build_success())
}

async fn start_media_stub(
    listener: TcpListener,
    cancel: CancellationToken,
) -> base::tokio::task::JoinHandle<()> {
    let app = Router::new()
        .route("/listen/media", post(listen_media))
        .route("/sdp/media", post(sdp_media))
        .route("/stream/online", post(stream_online))
        .route("/record/info", post(record_info))
        .route("/close/output", post(close_output))
        .route("/talk/open", post(talk_open))
        .route("/talk/answer", post(talk_answer))
        .route("/talk/close", post(talk_close))
        .with_state(Arc::new(MediaState::default()));
    base::tokio::spawn(async move {
        axum::serve(listener, app)
            .with_graceful_shutdown(cancel.cancelled_owned())
            .await
            .expect("run media stub");
    })
}

fn header_value<'a>(message: &'a str, name: &str) -> &'a str {
    message
        .lines()
        .find_map(|line| {
            let (header, value) = line.split_once(':')?;
            header.eq_ignore_ascii_case(name).then_some(value.trim())
        })
        .unwrap_or_else(|| panic!("missing {name} header"))
}

fn response_for(
    request: &str,
    status: u16,
    reason: &str,
    content_type: Option<&str>,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> String {
    let to = header_value(request, "To");
    let to = if to.contains(";tag=") {
        to.to_string()
    } else {
        format!("{to};tag=device-normal")
    };
    let mut response = format!(
        "SIP/2.0 {status} {reason}\r\n\
Via: {}\r\n\
From: {}\r\n\
To: {to}\r\n\
Call-ID: {}\r\n\
CSeq: {}\r\n",
        header_value(request, "Via"),
        header_value(request, "From"),
        header_value(request, "Call-ID"),
        header_value(request, "CSeq")
    );
    for (name, value) in extra_headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    if let Some(content_type) = content_type {
        response.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    response.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    response
}

fn device_message(call_id: &str, cseq: u32, body: &str) -> String {
    format!(
        "MESSAGE sip:{PLATFORM_ID}@192.0.2.10:25600 SIP/2.0\r\n\
Via: SIP/2.0/UDP 198.51.100.20:5060;rport;branch=z9hG4bK-{call_id}\r\n\
From: <sip:{DEVICE_ID}@3402000000>;tag=device-normal\r\n\
To: <sip:{PLATFORM_ID}@3402000000>\r\n\
Call-ID: {call_id}\r\n\
CSeq: {cseq} MESSAGE\r\n\
Contact: <sip:{DEVICE_ID}@198.51.100.20:5060>\r\n\
Max-Forwards: 70\r\n\
Content-Type: Application/MANSCDP+xml\r\n\
Content-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
}

fn register_packet() -> String {
    format!(
        "REGISTER sip:3402000000@192.0.2.10:25600 SIP/2.0\r\n\
Via: SIP/2.0/UDP 198.51.100.20:5060;branch=z9hG4bK-session-register;rport\r\n\
From: <sip:{DEVICE_ID}@3402000000>;tag=device-register\r\n\
To: <sip:{DEVICE_ID}@3402000000>\r\n\
Call-ID: session-normal-register\r\n\
CSeq: 1 REGISTER\r\n\
Contact: <sip:{DEVICE_ID}@198.51.100.20:5060>\r\n\
Expires: 3600\r\n\
User-Agent: GMV-Synthetic-Device/1.0\r\n\
X-GB-Ver: 3.0\r\n\
Max-Forwards: 70\r\n\
Content-Length: 0\r\n\r\n"
    )
}

async fn inject(socket: &UdpSocket, runtime_addr: SocketAddr, packet: String) {
    socket
        .send_to(packet.as_bytes(), runtime_addr)
        .await
        .expect("inject device packet");
    sleep(Duration::from_millis(5)).await;
}

async fn run_device(socket: Arc<UdpSocket>, runtime_addr: SocketAddr, cancel: CancellationToken) {
    let mut device_cseq = 100_u32;
    let mut buffer = vec![0; 65_535];
    loop {
        let received = base::tokio::select! {
            received = socket.recv_from(&mut buffer) => received,
            _ = cancel.cancelled() => break,
        };
        let (len, _) = received.expect("receive runtime SIP packet");
        let request = String::from_utf8_lossy(&buffer[..len]).into_owned();
        if request.starts_with("SIP/2.0 ") || request.starts_with("ACK ") {
            continue;
        }
        if request.starts_with("INVITE ") {
            let trying = response_for(&request, 100, "Trying", None, "", &[]);
            inject(&socket, runtime_addr, trying).await;
            let offer = request
                .split_once("\r\n\r\n")
                .map(|(_, body)| body)
                .unwrap_or_default();
            let ssrc = offer
                .lines()
                .find_map(|line| line.strip_prefix("y="))
                .unwrap_or("0100000001");
            let subject = header_value(&request, "Subject");
            let expected_subject_suffix = format!(":{ssrc}");
            let Some((source_subject, target_subject)) = subject.split_once(',') else {
                panic!("INVITE Subject missing two legs: {subject}");
            };
            assert!(
                source_subject.ends_with(&expected_subject_suffix),
                "source_subject={source_subject}; expected_suffix={expected_subject_suffix}; \
                 request={request}"
            );
            assert!(
                target_subject.starts_with(PLATFORM_ID),
                "target_subject={target_subject}; expected_media_server={PLATFORM_ID}"
            );
            let answer = if offer.contains("m=audio") {
                format!(
                    "v=0\r\n\
o={DEVICE_ID} 0 0 IN IP4 198.51.100.20\r\n\
s=Talk\r\n\
c=IN IP4 198.51.100.20\r\n\
t=0 0\r\n\
m=audio 30002 RTP/AVP 8\r\n\
a=sendrecv\r\n\
a=rtpmap:8 PCMA/8000\r\n\
y={ssrc}\r\n"
                )
            } else {
                format!(
                    "v=0\r\n\
o={DEVICE_ID} 0 0 IN IP4 198.51.100.20\r\n\
s=Play\r\n\
c=IN IP4 198.51.100.20\r\n\
t=0 0\r\n\
m=video 30000 RTP/AVP 96\r\n\
a=sendonly\r\n\
a=rtpmap:96 PS/90000\r\n\
y={ssrc}\r\n"
                )
            };
            let contact_addr = socket.local_addr().expect("synthetic device address");
            let contact = format!("<sip:{DEVICE_ID}@{contact_addr}>");
            let ok = response_for(
                &request,
                200,
                "OK",
                Some("application/sdp"),
                &answer,
                &[("Contact", contact.as_str())],
            );
            inject(&socket, runtime_addr, ok).await;
            continue;
        }
        if request.starts_with("SUBSCRIBE ") {
            let contact_addr = socket.local_addr().expect("synthetic device address");
            let contact = format!("<sip:{DEVICE_ID}@{contact_addr}>");
            let ok = response_for(
                &request,
                200,
                "OK",
                None,
                "",
                &[("Contact", contact.as_str()), ("Expires", "3600")],
            );
            inject(&socket, runtime_addr, ok).await;
            continue;
        }

        let is_snapshot =
            request.starts_with("MESSAGE ") && request.contains("<CmdType>DeviceConfig</CmdType>");
        let snapshot_session_id = is_snapshot
            .then(|| extract_xml_value_lossy(&request, "SessionID"))
            .flatten();
        let ok = response_for(&request, 200, "OK", None, "", &[]);
        inject(&socket, runtime_addr, ok).await;
        if let Some(session_id) = snapshot_session_id {
            device_cseq += 1;
            let body = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\
<Notify>\r\n\
<CmdType>UploadSnapshotFinished</CmdType>\r\n\
<SN>{device_cseq}</SN>\r\n\
<DeviceID>{DEVICE_ID}</DeviceID>\r\n\
<SessionID>{session_id}</SessionID>\r\n\
</Notify>\r\n"
            );
            inject(
                &socket,
                runtime_addr,
                device_message("snapshot-finished", device_cseq, &body),
            )
            .await;
        }
    }
}

async fn post_json<T: Serialize>(app: &Router, path: &str, value: &T, token: bool) -> Value {
    let body = base::serde_json::to_vec(value).expect("serialize HTTP request");
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if token {
        request = request.header("gmv-token", "normal-flow-token");
    }
    let response = app
        .clone()
        .oneshot(request.body(Body::from(body)).expect("build request"))
        .await
        .expect("call HTTP router");
    assert_eq!(response.status(), StatusCode::OK, "{path}");
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("read HTTP response");
    base::serde_json::from_slice(&bytes).expect("parse HTTP response")
}

fn assert_success(value: &Value, path: &str) {
    assert_eq!(value["code"], 200, "{path}: {value}");
}

fn base_stream_info(stream_id: &str) -> BaseStreamInfo {
    let (server_name, ssrc) =
        state::session::Cache::stream_map_query_node_ssrc(&stream_id.to_string())
            .expect("stream state exists");
    BaseStreamInfo {
        rtp_info: RtpInfo {
            ssrc,
            origin_trans: None,
            server_name,
            proxy_addr: "http://127.0.0.1:18570".into(),
        },
        stream_id: stream_id.to_string(),
        in_time: 1,
    }
}

fn prepare_config(media_port: u16, root: &PathBuf) -> PathBuf {
    let mut config = include_str!("../config.yml")
        .replace("51010000002000000001", PLATFORM_ID)
        .replace("5101000000", "3402000000")
        .replace("192.168.0.22", "192.0.2.10")
        .replace("local_port: 18570", &format!("local_port: {media_port}"));
    config = config.replace(
        "storage_path: ./videos/down",
        &format!("storage_path: {}/videos", root.display()),
    );
    config = config.replace(
        "storage_path: ./pics/raw",
        &format!("storage_path: {}/pics", root.display()),
    );
    let path = root.join("config.yml");
    std::fs::write(&path, config).expect("write normal flow config");
    path
}

#[test]
fn all_business_http_apis_complete_the_normal_signaling_flow() {
    let _runtime_guard = RUNTIME_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let runtime = base::tokio::runtime::Runtime::new().expect("create Tokio runtime");
    runtime.block_on(async {
        let root = std::env::temp_dir().join(format!("gmv-normal-flow-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("create normal flow temp directory");

        let media_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .expect("bind media stub");
        let media_port = media_listener
            .local_addr()
            .expect("media stub address")
            .port();
        let config_path = prepare_config(media_port, &root);
        init_cfg(config_path.to_string_lossy().into_owned());

        let _storage_guard = enable_test_storage(GmvOauth {
            device_id: DEVICE_ID.into(),
            domain_id: PLATFORM_ID.into(),
            domain: "3402000000".into(),
            pwd: None,
            pwd_check: 0,
            alias: Some("normal-flow-device".into()),
            status: 1,
            heartbeat_sec: 60,
        });

        let cancel = CancellationToken::new();
        let media_task = start_media_stub(media_listener, cancel.child_token()).await;
        db_task::init(cancel.child_token());
        let session_conf = SessionConf::get_session_by_conf();
        Register::init(session_conf.clone(), cancel.child_token()).expect("initialize Register");
        let auth_cache = auth::init_global().await.expect("initialize auth cache");
        let runtime_udp =
            std::net::UdpSocket::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind SIP runtime UDP");
        let runtime_addr = runtime_udp.local_addr().expect("runtime UDP local address");
        let device_socket = Arc::new(
            UdpSocket::bind((Ipv4Addr::LOCALHOST, 0))
                .await
                .expect("bind synthetic device UDP"),
        );
        let device_addr = device_socket
            .local_addr()
            .expect("device UDP local address");
        let (service, _events) = NativeSipRuntimeService::start(
            Ipv4Addr::LOCALHOST,
            runtime_addr.port(),
            session_conf.domain.clone(),
            SipRuntimeSockets {
                udp: Some(runtime_udp),
                tcp: None,
                tls: None,
            },
            auth_cache,
            cancel.child_token(),
        )
        .expect("start native SIP service");
        let handle = service.handle();
        handle.install_global().expect("install native runtime");
        let device_task = base::tokio::spawn(run_device(
            device_socket.clone(),
            runtime_addr,
            cancel.child_token(),
        ));
        inject(&device_socket, runtime_addr, register_packet()).await;
        timeout(Duration::from_secs(3), async {
            while !Register::has_session(DEVICE_ID) {
                sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("REGISTER completes");

        command::query_device_info(DEVICE_ID, 38)
            .await
            .expect("query device info");
        command::query_catalog(DEVICE_ID, 39)
            .await
            .expect("query catalog");
        command::query_record_info(DEVICE_ID, 40, "2026-06-13T00:00:00", "2026-06-13T01:00:00")
            .await
            .expect("query record info");
        command::query_preset(DEVICE_ID)
            .await
            .expect("query preset");

        let app = http::routes();
        let live = post_json(
            &app,
            "/api/play/live/stream",
            &json!({
                "device_id": DEVICE_ID,
                "channel_id": CHANNEL_ID,
                "trans_mode": null,
                "custom_media_config": null
            }),
            true,
        )
        .await;
        assert_success(&live, "/api/play/live/stream");
        let live_id = live["data"]["streamId"]
            .as_str()
            .expect("live stream id")
            .to_string();
        let live_base = base_stream_info(&live_id);
        for (path, body) in [
            (
                "/hook/stream/register",
                json!({"base_stream_info": live_base, "code": 200, "msg": null}),
            ),
            (
                "/hook/on/play",
                json!({
                    "base_stream_info": base_stream_info(&live_id),
                    "remote_addr": "198.51.100.30:40000",
                    "token": "normal-flow-token",
                    "play_type": OutputEnum::DashFmp4
                }),
            ),
            (
                "/hook/off/play",
                json!({
                    "base_stream_info": base_stream_info(&live_id),
                    "remote_addr": "198.51.100.30:40000",
                    "token": "normal-flow-token",
                    "play_type": OutputEnum::DashFmp4
                }),
            ),
            (
                "/hook/stream/idle",
                json!({
                    "base_stream_info": base_stream_info(&live_id),
                    "play_type": OutputEnum::DashFmp4,
                    "user_count": 0
                }),
            ),
        ] {
            let value = post_json(&app, path, &body, false).await;
            assert_success(&value, path);
        }
        state::session::Cache::stream_map_remove(&live_id, None);

        let playback = post_json(
            &app,
            "/api/play/back/stream",
            &json!({
                "device_id": DEVICE_ID,
                "channel_id": PLAYBACK_CHANNEL_ID,
                "trans_mode": null,
                "custom_media_config": null,
                "st": 1781308800_u32,
                "et": 1781312400_u32
            }),
            true,
        )
        .await;
        assert_success(&playback, "/api/play/back/stream");
        let playback_id = playback["data"]["streamId"]
            .as_str()
            .expect("playback stream id")
            .to_string();
        for (path, body) in [
            (
                "/api/play/back/seek",
                json!({"streamId": playback_id, "seekSecond": 30}),
            ),
            (
                "/api/play/back/speed",
                json!({"streamId": playback_id, "speedRate": 2.0}),
            ),
        ] {
            let value = post_json(&app, path, &body, true).await;
            assert_success(&value, path);
        }
        let timeout_hook = post_json(
            &app,
            "/hook/stream/input/timeout",
            &json!({
                "base_stream_info": base_stream_info(&playback_id),
                "user_count": 0
            }),
            false,
        )
        .await;
        assert_success(&timeout_hook, "/hook/stream/input/timeout");

        let ptz = post_json(
            &app,
            "/api/control/ptz",
            &json!({
                "deviceId": DEVICE_ID,
                "channelId": CHANNEL_ID,
                "leftRight": 1,
                "upDown": 0,
                "inOut": 0,
                "horizonSpeed": 32,
                "verticalSpeed": 16,
                "zoomSpeed": 0
            }),
            true,
        )
        .await;
        assert_success(&ptz, "/api/control/ptz");

        let download = post_json(
            &app,
            "/api/download/mp4",
            &json!({
                "device_id": DEVICE_ID,
                "channel_id": CHANNEL_ID,
                "trans_mode": null,
                "custom_media_config": null,
                "st": 1781308800_u32,
                "et": 1781312400_u32
            }),
            true,
        )
        .await;
        assert_success(&download, "/api/download/mp4");
        let download_id = download["data"]
            .as_str()
            .expect("download stream id")
            .to_string();
        let downing = post_json(
            &app,
            "/api/downing/info",
            &json!({"stream_id": download_id, "media_type": null}),
            true,
        )
        .await;
        assert_success(&downing, "/api/downing/info");
        let record_path = root
            .join("record")
            .join("20260613")
            .join(format!("{download_id}.mp4"));
        let end_record = post_json(
            &app,
            "/hook/end/record",
            &json!({
                "path_file_name": record_path,
                "file_size": 4096,
                "timestamp": 3600,
                "state": 2
            }),
            false,
        )
        .await;
        assert_success(&end_record, "/hook/end/record");
        let stop_download = post_json(
            &app,
            "/api/download/stop",
            &json!({"param": download_id}),
            true,
        )
        .await;
        assert_success(&stop_download, "/api/download/stop");

        let snapshot = post_json(
            &app,
            "/edge/snapshot/image",
            &json!({
                "device_channel_ident": {
                    "device_id": DEVICE_ID,
                    "channel_id": CHANNEL_ID
                },
                "count": 1
            }),
            true,
        )
        .await;
        assert_success(&snapshot, "/edge/snapshot/image");
        let snapshot_session = snapshot["data"].as_str().expect("snapshot session id");
        let token = edge_token::test_token_for_session_id(snapshot_session);
        let mut image = Cursor::new(Vec::new());
        DynamicImage::new_rgb8(1, 1)
            .write_to(&mut image, ImageFormat::Png)
            .expect("encode test image");
        let upload_uri = format!(
            "/edge/upload/picture/{token}?SessionID={snapshot_session}&fileId=normal-snapshot"
        );
        let upload_response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(upload_uri)
                    .header("content-type", "image/png")
                    .body(Body::from(image.into_inner()))
                    .expect("build upload request"),
            )
            .await
            .expect("upload snapshot");
        assert_eq!(upload_response.status(), StatusCode::OK);
        let snapshot_file_id =
            test_file_id_by_biz_id(snapshot_session).expect("snapshot file metadata");
        let remove_file = post_json(
            &app,
            "/api/rm/file",
            &json!({"param": snapshot_file_id}),
            true,
        )
        .await;
        assert_success(&remove_file, "/api/rm/file");

        let talk = post_json(
            &app,
            "/api/talk/start",
            &json!({
                "device_id": DEVICE_ID,
                "channel_id": CHANNEL_ID,
                "transport": "udp",
                "codec": "PCMA",
                "sample_rate": 8000,
                "channel_count": 1,
                "frame_duration_ms": 20
            }),
            true,
        )
        .await;
        assert_success(&talk, "/api/talk/start");
        let talk_id = talk["data"]["talk_id"]
            .as_str()
            .expect("talk id")
            .to_string();
        let stop_talk = post_json(&app, "/api/talk/stop", &json!({"talk_id": talk_id}), true).await;
        assert_success(&stop_talk, "/api/talk/stop");
        let closed = post_json(
            &app,
            "/hook/talk/closed",
            &json!({"talk_id": talk_id, "reason": "normal"}),
            false,
        )
        .await;
        assert_success(&closed, "/hook/talk/closed");

        let command_close = post_json(
            &app,
            "/api/play/live/stream",
            &json!({
                "device_id": DEVICE_ID,
                "channel_id": "34020000001320000103",
                "trans_mode": null,
                "custom_media_config": null
            }),
            true,
        )
        .await;
        assert_success(&command_close, "/api/play/live/stream");
        let command_close_id = command_close["data"]["streamId"]
            .as_str()
            .expect("command close stream id")
            .to_string();
        let command_close_call_id = state::session::Cache::stream_call_id(&command_close_id)
            .expect("command close call id");
        command::invite_stop_by_stream(&command_close_id)
            .await
            .expect("stop stream by business index");
        assert!(
            state::session::Cache::stream_terminated_by_call_id(&command_close_call_id).is_some(),
            "remove command close stream state"
        );

        sleep(Duration::from_millis(100)).await;
        cancel.cancel();
        device_task.await.expect("stop device simulator");
        service.shutdown();
        media_task.await.expect("stop media stub");
        std::fs::remove_dir_all(&root).expect("remove normal flow temp directory");
    });
}
