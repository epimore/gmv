use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::oneshot::Sender;
use crate::biz::call::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};

//对外发送事件
pub enum OutEvent {
    //流媒体触发事件，回调信令
    StreamIn(BaseStreamInfo),
    StreamOutIdle(BaseStreamInfo),
    StreamInTimeout(StreamState),
    StreamUnknown(RtpInfo),
    OnPlay(StreamPlayInfo),
    OffPlay(StreamPlayInfo),
    EndRecord(StreamRecordInfo),
}

//None-未响应或响应超时等异常
pub enum OutEventRes {
    //收到国标媒体流事件：响应内容不敏感;some-成功接收;None-未成功接收
    StreamIn(Option<bool>),
    //无人观看时，关闭流
    StreamOutIdle(Option<u8>),
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
        while let Some((event, tx)) = rx.recv().await {
            match event {
                OutEvent::StreamIn(bsi) => {
                    bsi.stream_in().await;
                }
                OutEvent::StreamOutIdle(bsi) => {
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
                OutEvent::EndRecord(_) => {}
            }
        }
    }
}

//接收外部事件
#[derive(Copy, Clone)]
pub enum InEvent {
    //信令回到流媒体事件
    MediaInit(),
    //流注册
    StreamIn(),
}
