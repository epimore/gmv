use std::net::SocketAddr;
use std::sync::Arc;
use crate::io::http::call::{HttpClient, HttpSession};
use crate::io::local::mp4::LocalStoreMp4Context;
use crate::state::layer::output_layer::OutputLayer;
use crate::state::register::{Inner, Register, TimeScheduleKey};
use base::cache::c100k::CacheEvent;
use base::exception::GlobalResultExt;
use base::log::{error, info};
use base::tokio::select;
use base::tokio::sync::mpsc::Receiver;
use base::tokio::sync::oneshot::Sender;
use base::tokio_util::sync::CancellationToken;
use pretend::Pretend;
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use shared::info::obj::{BaseStreamInfo, InTimeoutEventRes, OutputEventRes, OutputStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};
use std::sync::atomic::Ordering;
use std::time::Duration;
use base::net::state::Protocol;
use base::tokio::sync::mpsc;
use pretend_reqwest::Client;

pub enum Event {
    Out(OutEvent),
    Active(ActiveEvent),
    Inner(InnerEvent),

}

pub enum InnerEvent {
    RecordInfo(StreamRecordInfo),
    //rtp_type,stream_id
    StreamRegister(u8,Arc<str>,(SocketAddr, Protocol)),
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
    StreamInvalid(BaseStreamInfo),
    StreamInTimeout(StreamState),
    StreamIdle(OutputStreamInfo),
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
        pretend:&Pretend<Client, UrlResolver, NoopRequestInterceptor>
    ) {
        if let Some((event, tx)) = rx.recv().await {
            match event {
                Event::Out(out) => {
                    Self::hand_out(out, tx, pretend).await;
                }
                Event::Active(active) => {
                    Self::hand_active(active,tx).await;
                },
                Event::Inner(inner) => {

                }
            }
        }
    }

    async fn hand_inner(inner_event: InnerEvent, tx: Option<Sender<EventRes>>,pretend: &Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>,){
        match inner_event {
            InnerEvent::RecordInfo(_) => {unimplemented!()}
            InnerEvent::StreamRegister(rtp_type, stream_id,origin_trans) => {
                //当不存在则表示数据被释放；统一由时间调度触发OutEvent::StreamInTimeout
                //1.insert remote addr + protocol
                if Register::insert_origin_trans(stream_id.clone(), origin_trans) {
                    //2.send stream_config to muxer
                    let _ = Register::send_stream_config(rtp_type,stream_id.clone());
                    //3.send OutEvent::StreamRegister
                    match Register::build_stream_info(stream_id) {
                        None => {}
                        Some(info) => {
                            info!("Calling stream_register with: {:?}", info);
                            let res = pretend.stream_register(&info).await;
                            info!("stream_register returned: {:?}", res);
                            let _ = res.hand_log(|msg| error!("{msg}"));
                        }
                    }
                }
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
                let mut oe = InTimeoutEventRes::CloseAll;
                if let Ok(oer) = res {
                    let resp = oer.value();
                    if let Some(oer) = resp.data && resp.code==200 {
                        oe = oer;
                    }
                }
                Register::close_stream_by_input(ss, oe);
                // let _ = res.hand_log(|msg| error!("{msg}"));
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
            OutEvent::StreamIdle(os) => {
                info!("Calling stream_idle with: {:?}", os);
                let res = pretend.stream_idle(&os).await;
                info!("stream_idle returned: {:?}", res);
                let mut oe = if os.user_count ==0 {
                    OutputEventRes::CloseAll
                }else {
                    OutputEventRes::CloseMuxer
                };
                if let Ok(oer) = res {
                    let resp = oer.value();
                    if let Some(oer) = resp.data && resp.code==200 {
                        oe = oer;
                    }
                }
                Register::close_stream_by_output(os, oe);
                // let _ = res.hand_log(|msg| error!("{msg}"));
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
            OutEvent::StreamInvalid(info) => {
                info!("Calling stream_invalid with: {:?}", info);
                let res = pretend.stream_invalid(&info).await;
                info!("stream_invalid returned: {:?}", res);
                let _ = res.hand_log(|msg| error!("{msg}"));
            }
        }
    }
}
pub async fn schedule_event(inner: Arc<Inner>, mut event_rx:mpsc::Receiver<(Event, Option<Sender<EventRes>>)>, cancel_token: CancellationToken) {
    let pretend = HttpClient::template()
        .as_ref()
        .expect("Http client template init failed");
    loop {
        select! {
           biased; // 按编写顺序检查分支
            _ = on_time_schedule(&inner)=>{},
            _ = Event::hand_event(&mut event_rx,&pretend) => {}
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
                    Register::handle_rtp_in_timeout(ssrc,inner);
                }
                TimeScheduleKey::OutSession(expire_id) => {
                    Register::clean_play_token(expire_id);
                }
            }
        }
    }
}
