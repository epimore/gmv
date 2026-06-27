use crate::guard_integration::{check_playback, publish_guard_event};
use crate::io::call::{call_session_hook_rpc, decode_hook_payload};
use crate::io::local::mp4::LocalStoreMp4Context;
use crate::state::layer::output_layer::OutputLayer;
use crate::state::register::{Inner, Register, TimeScheduleKey};
use base::cache::c100k::CacheEvent;
use base::exception::GlobalResultExt;
use base::log::{error, info, warn};
use base::net::state::Protocol;
use base::serde::Serialize;
use base::tokio;
use base::tokio::select;
use base::tokio::sync::mpsc::Receiver;
use base::tokio::sync::oneshot::Sender;
use base::tokio::sync::{Semaphore, mpsc};
use base::tokio_util::sync::CancellationToken;
use gmv_domain::info::obj::{
    BaseStreamInfo, InTimeoutEventRes, OutputEventRes, OutputStreamInfo, RegisterStreamInfo,
    RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState, UnknownStreamEvent,
};
use gmv_protocol::session::v1::SessionHookResponse;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::Ordering;

const MAX_WORKER_POOL: usize = 128;
pub enum Event {
    Out(OutEvent),
    Active(ActiveEvent),
    Inner(InnerEvent),
}

pub enum InnerEvent {
    RecordInfo(StreamRecordInfo),
    //rtp_type,stream_id
    StreamRegister(u8, Arc<str>, (SocketAddr, Protocol)),
}
pub enum ActiveEvent {
    RtmpPush(u32),
    LocalStoreMp4(LocalStoreMp4Context),
    LocalStoreTs(u32),
    RtspPush(u32),
    Gb28181Push(u32),
    WebRtcPush(u32),
}

/// 主动推流：【rtmp-push,local-mp4/ts,rtsp-push,gb28181-push,webRtc-push】
/// 两种方式触发
/// 1.初始化的媒体流检测是否需主动推流，
/// 2.通过API接口添加的输出流OUTPUT，检测是否需主动推流
///
/// API添加流注册，可确定outputKind
fn event_push_stream(output: &OutputLayer) {}
//对外发送事件
pub enum OutEvent {
    //流媒体触发事件，回调信令
    StreamRegister(RegisterStreamInfo),
    StreamInTimeout(StreamState),
    StreamIdle(OutputStreamInfo),
    StreamUnknown(UnknownStreamEvent),
    OnPlay(StreamPlayInfo),
    OffPlay(StreamPlayInfo),
    EndRecord(StreamRecordInfo),
}
pub enum EventRes {
    Out(OutEventRes),
    Inner(InnerEventRes),
}
pub enum InnerEventRes {}
//None-未响应或响应超时等异常
pub enum OutEventRes {
    //收到国标媒体流事件：响应内容不敏感;some-成功接收;None-未成功接收
    StreamRegister(Option<()>),
    //接收国标媒体流超时事件：取消监听该SSRC,响应内容不敏感;
    StreamInTimeout(Option<InTimeoutEventRes>),
    //流闲置或无人观看时:
    StreamIdle(Option<OutputEventRes>),
    //未知ssrc流事件；响应内容不敏感,some-成功接收;None-未成功接收
    StreamUnknown(Option<()>),
    //用户点播媒体流事件,none与false-回复用户401，true-写入流
    OnPlay(Option<bool>),
    //用户关闭媒体流事件;响应内容不敏感,some-成功接收;None-未成功接收
    OffPlay(Option<()>),
    //录像完成事件：响应内容不敏感;some-成功接收;None-未成功接收->查看流是否被使用(观看)->否->调用streamIdle事件
    EndRecord(Option<()>),
}

impl Event {
    async fn hand_event(
        rx: &mut Receiver<(Event, Option<Sender<EventRes>>)>,
        semaphore: Arc<Semaphore>,
    ) {
        if let Some((event, tx)) = rx.recv().await {
            match event {
                Event::Out(out) => {
                    if let Ok(permit) = semaphore
                        .acquire_owned()
                        .await
                        .hand_log(|msg| error!("{msg}"))
                    {
                        tokio::spawn(async move {
                            Self::hand_out(out, tx).await;
                            drop(permit);
                        });
                    }
                }
                Event::Active(active) => {
                    Self::hand_active(active, tx);
                }
                Event::Inner(inner) => {
                    Self::hand_inner(inner, tx);
                }
            }
        }
    }

    fn hand_inner(inner_event: InnerEvent, tx: Option<Sender<EventRes>>) {
        match inner_event {
            InnerEvent::RecordInfo(_) => {
                warn!("stream ignored unsupported record-info inner event");
            }
            InnerEvent::StreamRegister(rtp_type, stream_id, origin_trans) => {
                //当不存在则表示数据被释放；统一由时间调度触发OutEvent::StreamInTimeout
                //1.insert remote addr + protocol
                if Register::insert_origin_trans(stream_id.clone(), origin_trans) {
                    //2.send stream_config to muxer and call session register
                    let _ = Register::send_stream_config(rtp_type, stream_id.clone());
                }
            }
        }
    }

    fn hand_active(active_event: ActiveEvent, tx: Option<Sender<EventRes>>) {
        match active_event {
            ActiveEvent::RtmpPush(_) => {}
            ActiveEvent::LocalStoreMp4(ctx) => {
                ctx.store();
            }
            ActiveEvent::LocalStoreTs(_) => {}
            ActiveEvent::RtspPush(_) => {}
            ActiveEvent::Gb28181Push(_) => {}
            ActiveEvent::WebRtcPush(_) => {}
        }
    }

    async fn call_session_hook_by_stream_id<T>(
        stream_id: &str,
        event_type: &str,
        payload: &T,
    ) -> Option<SessionHookResponse>
    where
        T: Serialize + ?Sized,
    {
        let Some(endpoint) = Register::session_hook_endpoint(stream_id) else {
            warn!("session hook endpoint missing: stream_id={stream_id}, event_type={event_type}");
            return None;
        };
        call_session_hook_rpc(&endpoint, event_type, payload).await
    }

    fn accepted(response: Option<&SessionHookResponse>) -> bool {
        response.map(|response| response.accepted).unwrap_or(false)
    }

    async fn hand_out(out_event: OutEvent, tx: Option<Sender<EventRes>>) {
        match out_event {
            OutEvent::StreamRegister(rsi) => {
                let stream_id = rsi.base_stream_info.stream_id.clone();
                let response =
                    Self::call_session_hook_by_stream_id(&stream_id, "stream.registered", &rsi)
                        .await;
                if Self::accepted(response.as_ref()) {
                    info!("stream_register event sent to session: {:?}", rsi);
                } else {
                    publish_guard_event(
                        "stream.registered.fallback",
                        format!("{rsi:?}").into_bytes(),
                    );
                    warn!("stream_register fallback event sent to guard: {:?}", rsi);
                }
                if let Some(tx) = tx {
                    let result = Self::accepted(response.as_ref()).then_some(());
                    let _ = tx.send(EventRes::Out(OutEventRes::StreamRegister(result)));
                }
            }
            OutEvent::StreamInTimeout(ss) => {
                let stream_id = ss.base_stream_info.stream_id.clone();
                let response =
                    Self::call_session_hook_by_stream_id(&stream_id, "stream.input_timeout", &ss)
                        .await;
                let event_res = response
                    .as_ref()
                    .filter(|response| response.accepted)
                    .and_then(decode_hook_payload::<InTimeoutEventRes>)
                    .unwrap_or_else(|| {
                        publish_guard_event(
                            "stream.input_timeout.fallback",
                            format!("{ss:?}").into_bytes(),
                        );
                        warn!("stream_input_timeout fallback close all: stream_id={stream_id}");
                        InTimeoutEventRes::CloseAll
                    });
                Register::close_stream_by_input(ss, event_res);
            }
            OutEvent::OnPlay(spi) => {
                let stream_id = spi.base_stream_info.stream_id.clone();
                let accepted = check_playback(
                    &stream_id,
                    &spi.token,
                    spi.remote_addr.as_deref(),
                    &format!("{:?}", spi.play_type),
                )
                .await;
                if !accepted {
                    publish_guard_event("stream.on_play.rejected", format!("{spi:?}").into_bytes());
                }
                if let Some(tx) = tx {
                    let _ = tx.send(EventRes::Out(OutEventRes::OnPlay(Some(accepted))));
                }
            }
            OutEvent::StreamIdle(os) => {
                let stream_id = os.base_stream_info.stream_id.clone();
                let response =
                    Self::call_session_hook_by_stream_id(&stream_id, "stream.idle", &os).await;
                let event_res = response
                    .as_ref()
                    .filter(|response| response.accepted)
                    .and_then(decode_hook_payload::<OutputEventRes>)
                    .unwrap_or_else(|| {
                        publish_guard_event("stream.idle.fallback", format!("{os:?}").into_bytes());
                        warn!("stream_idle fallback local cleanup: stream_id={stream_id}");
                        if os.user_count == 0 {
                            OutputEventRes::CloseAll
                        } else {
                            OutputEventRes::CloseMuxer
                        }
                    });
                Register::close_stream_by_output(os, event_res);
            }
            OutEvent::StreamUnknown(event) => {
                publish_guard_event("stream.unknown", format!("{event:?}").into_bytes());
                info!(
                    "stream_unknown event sent to guard: media_node={}, ssrc={}",
                    event.media_node_id, event.ssrc
                );
            }
            OutEvent::OffPlay(spi) => {
                let stream_id = spi.base_stream_info.stream_id.clone();
                let response =
                    Self::call_session_hook_by_stream_id(&stream_id, "stream.off_play", &spi).await;
                if Self::accepted(response.as_ref()) {
                    info!("off_play event sent to session: {:?}", spi);
                } else {
                    publish_guard_event(
                        "stream.off_play.fallback",
                        format!("{spi:?}").into_bytes(),
                    );
                    warn!("off_play fallback event sent to guard: {:?}", spi);
                }
            }
            OutEvent::EndRecord(info) => {
                let response = match info.stream_id.as_deref() {
                    Some(stream_id) => {
                        Self::call_session_hook_by_stream_id(stream_id, "stream.end_record", &info)
                            .await
                    }
                    None => {
                        warn!("end_record stream_id missing");
                        None
                    }
                };
                if Self::accepted(response.as_ref()) {
                    info!("end_record event sent to session: {:?}", info);
                } else {
                    publish_guard_event(
                        "stream.end_record.fallback",
                        format!("{info:?}").into_bytes(),
                    );
                    warn!("end_record fallback event sent to guard: {:?}", info);
                }
            }
        }
    }
}
pub async fn schedule_event(
    inner: Arc<Inner>,
    mut event_rx: Receiver<(Event, Option<Sender<EventRes>>)>,
    cancel_token: CancellationToken,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_WORKER_POOL));
    loop {
        select! {
           biased; // 按编写顺序检查分支
            _ = on_time_schedule(&inner)=>{},
            _ = Event::hand_event(&mut event_rx,semaphore.clone()) => {}
            _ = cancel_token.cancelled() => {break;}
        }
    }
}

//let s = String::from("abc");
// let arc: Arc<str> = Arc::from(s);
async fn on_time_schedule(inner: &Inner) {
    if let Some(batch) = inner.time_schedule.next_batch().await {
        for CacheEvent { key, version, .. } in batch {
            match key {
                TimeScheduleKey::RtpGateway(ssrc) => {
                    Register::handle_rtp_in_timeout(ssrc, inner);
                }
                TimeScheduleKey::OutSession(expire_id) => {
                    Register::clean_play_token(expire_id);
                }
                TimeScheduleKey::UnknownStream(key) => {
                    Register::expire_unknown_stream(key, inner);
                }
            }
        }
    }
}
