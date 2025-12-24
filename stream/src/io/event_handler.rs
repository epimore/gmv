use crate::io::http::call::{HttpClient, HttpSession};
use crate::io::local::mp4::LocalStoreMp4Context;
use crate::state::layer::output_layer::OutputLayer;
use base::exception::GlobalResultExt;
use base::log::{error, info};
use base::tokio::select;
use base::tokio::sync::mpsc::Receiver;
use base::tokio::sync::oneshot::Sender;
use base::tokio_util::sync::CancellationToken;
use pretend::Pretend;
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use shared::info::obj::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};

pub enum Event {
    Out(OutEvent),
    Active(ActiveEvent),
    Inner(InnerEvent),
}

pub enum InnerEvent {
    RecordInfo(StreamRecordInfo),
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
    StreamRegister(BaseStreamInfo),
    StreamInTimeout(StreamState),
    StreamIdle(BaseStreamInfo),
    StreamUnknown(RtpInfo),
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
    StreamInTimeout(Option<()>),
    //无人观看时,响应内容不敏感
    StreamIdle(Option<()>),
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
    pub async fn event_loop(
        mut rx: Receiver<(Event, Option<Sender<EventRes>>)>,
        cancel_token: CancellationToken,
    ) {
        let pretend = HttpClient::template()
            .as_ref()
            .expect("Http client template init failed");
        loop {
            select! {
               biased; // 按编写顺序检查分支
               Some((event, tx)) = rx.recv() =>{
                    match event {
                        Event::Out(out) => {
                            Self::hand_out(out, tx, pretend).await;
                        }
                        Event::Active(active) => {
                            Self::hand_active(active,tx).await;
                        },
                        Event::Inner(inner) => todo!()
                    }
                },
                _ = cancel_token.cancelled() => {break;}
            }
        }
    }
    async fn hand_active(active_event: ActiveEvent, tx: Option<Sender<EventRes>>) {
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

    async fn hand_out(
        out_event: OutEvent,
        tx: Option<Sender<EventRes>>,
        pretend: &Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>,
    ) {
        match out_event {
            OutEvent::StreamRegister(bsi) => {
                info!("Calling stream_register with: {:?}", bsi);
                let res = pretend.stream_register(&bsi).await;
                info!("stream_register returned: {:?}", res);
                let _ = res.hand_log(|msg| error!("{msg}"));
            }
            OutEvent::StreamInTimeout(ss) => {
                info!("Calling stream_input_timeout with: {:?}", ss);
                let res = pretend.stream_input_timeout(&ss).await;
                info!("stream_input_timeout returned: {:?}", res);
                let _ = res.hand_log(|msg| error!("{msg}"));
            }
            OutEvent::OnPlay(spi) => {
                info!("Calling on_play with: {:?}", spi);
                let res = pretend.on_play(&spi).await;
                info!("on_play returned: {:?}", res);
                if let Ok(res) = res.hand_log(|msg| error!("{msg}")) {
                    let _ = tx
                        .unwrap()
                        .send(EventRes::Out(OutEventRes::OnPlay(res.value().data)));
                }
            }
            OutEvent::StreamIdle(bsi) => {
                info!("Calling stream_idle with: {:?}", bsi);
                let res = pretend.stream_idle(&bsi).await;
                info!("stream_idle returned: {:?}", res);
                let _ = res.hand_log(|msg| error!("{msg}"));
            }
            OutEvent::StreamUnknown(_) => {}
            OutEvent::OffPlay(spi) => {
                info!("Calling off_play with: {:?}", spi);
                let res = pretend.off_play(&spi).await;
                info!("off_play returned: {:?}", res);
                let _ = res.hand_log(|msg| error!("{msg}"));
            }
            OutEvent::EndRecord(info) => {
                info!("Calling end_record with: {:?}", info);
                let res = pretend.end_record(&info).await;
                info!("end_record returned: {:?}", res);
                let _ = res.hand_log(|msg| error!("{msg}"));
            }
        }
    }
}
