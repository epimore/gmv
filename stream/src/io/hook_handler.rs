use common::exception::{GlobalResultExt};
use common::log::{error, info};
use common::serde::{Deserialize, Serialize};
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::oneshot::Sender;
use crate::biz::call::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};
use crate::container::hls::HlsPiece;
use crate::io::http::call::{HttpClient, HttpSession};

//对外发送事件
pub enum OutEvent {
    //流媒体触发事件，回调信令
    StreamRegister(BaseStreamInfo),
    StreamIdle(BaseStreamInfo),
    StreamInTimeout(StreamState),
    StreamUnknown(RtpInfo),
    OnPlay(StreamPlayInfo),
    OffPlay(StreamPlayInfo),
    EndRecord(StreamRecordInfo),
}

//None-未响应或响应超时等异常
pub enum OutEventRes {
    //收到国标媒体流事件：响应内容不敏感;some-成功接收;None-未成功接收
    StreamRegister(Option<bool>),
    //无人观看时，关闭流
    StreamIdle(Option<u8>),
    //接收国标媒体流超时事件：取消监听该SSRC,响应内容不敏感;
    StreamInTimeout(Option<bool>),
    //未知ssrc流事件；响应内容不敏感,some-成功接收;None-未成功接收
    StreamUnknown(Option<bool>),
    //用户点播媒体流事件,none与false-回复用户401，true-写入流
    OnPlay(Option<bool>),
    //用户关闭媒体流事件;响应内容不敏感,some-成功接收;None-未成功接收
    OffPlay(Option<bool>),
    //录像完成事件：响应内容不敏感;some-成功接收;None-未成功接收->查看流是否被使用(观看)->否->调用streamIdle事件
    EndRecord(Option<bool>),
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
                OutEvent::StreamIdle(bsi) => {
                    bsi.stream_idle().await;
                }
                OutEvent::StreamInTimeout(ss) => {
                    let _ = ss.stream_input_timeout().await;
                }
                OutEvent::StreamUnknown(_) => {}
                OutEvent::OnPlay(spi) => {
                    let res = spi.on_play().await;
                    let _ = tx.unwrap().send(OutEventRes::OnPlay(res));
                }
                OutEvent::OffPlay(spi) => {
                    let _ = spi.off_play().await;
                }
                OutEvent::EndRecord(info) => {
                    let _ = info.end_record().await;
                }
            }
        }
    }
}

//接收外部事件
#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "common::serde")]
pub enum InEvent {
    SessionEvent(SessionEvent),
    RtpStreamEvent(RtpStreamEvent),
}

//RTP 实时流事件
#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "common::serde")]
pub enum RtpStreamEvent {
    //流注册
    StreamRegister,
}

//SESSION 信令事件
#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "common::serde")]
pub enum SessionEvent {
    //媒体信息初始化
    MediaInit,
    MediaAction(MediaAction),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum MediaAction {
    //点播
    Play(Play),
    //下载
    Download(Download),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum Download {
    //录像 filename,type
    Mp4(String, Option<String>),
    //截图 filename
    Picture(String, Option<String>),
}

#[derive(Clone, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub enum Play {
    Flv,
    Hls(HlsPiece),
    FlvHls(HlsPiece),
}
