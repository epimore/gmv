use common::exception::{GlobalResultExt};
use common::log::{error, info};
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::oneshot::Sender;
use shared::info::obj::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};
use crate::io::http::call::{HttpClient, HttpSession};

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

//None-未响应或响应超时等异常
pub enum OutEventRes {
    //收到国标媒体流事件：响应内容不敏感;some-成功接收;None-未成功接收
    StreamRegister(Option<()>),
    //接收国标媒体流超时事件：取消监听该SSRC,响应内容不敏感;
    StreamInTimeout(Option<()>),
    //无人观看时，关闭流
    StreamIdle(Option<u8>),
    //未知ssrc流事件；响应内容不敏感,some-成功接收;None-未成功接收
    StreamUnknown(Option<()>),
    //用户点播媒体流事件,none与false-回复用户401，true-写入流
    OnPlay(Option<bool>),
    //用户关闭媒体流事件;响应内容不敏感,some-成功接收;None-未成功接收
    OffPlay(Option<()>),
    //录像完成事件：响应内容不敏感;some-成功接收;None-未成功接收->查看流是否被使用(观看)->否->调用streamIdle事件
    EndRecord(Option<()>),
}

impl OutEvent {
    pub async fn event_loop(mut rx: Receiver<(OutEvent, Option<Sender<OutEventRes>>)>) {
        let pretend = HttpClient::template().as_ref().expect("Http client template init failed");
        while let Some((event, tx)) = rx.recv().await {
            match event {
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
                        let _ = tx.unwrap().send(OutEventRes::OnPlay(res.value().data));
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
}