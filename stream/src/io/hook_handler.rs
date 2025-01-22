use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::oneshot::Sender;
use crate::biz::call::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};

pub enum Event {
    StreamIn(BaseStreamInfo),
    StreamOutIdle(BaseStreamInfo),
    StreamInTimeout(StreamState),
    StreamUnknown(RtpInfo),
    OnPlay(StreamPlayInfo),
    OffPlay(StreamPlayInfo),
    EndRecord(StreamRecordInfo),
}

//None-未响应或响应超时等异常
pub enum EventRes {
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

impl Event {
    pub async fn event_loop(mut rx: Receiver<(Event, Option<Sender<EventRes>>)>) {
        while let Some((event, tx)) = rx.recv().await {
            match event {
                Event::StreamIn(bsi) => {
                    bsi.stream_in().await;
                }
                Event::StreamOutIdle(bsi) => {
                    bsi.stream_idle().await;
                }
                Event::StreamInTimeout(ss) => {
                    let _ = ss.stream_input_timeout().await;
                }
                Event::StreamUnknown(_) => {}
                Event::OnPlay(spi) => {
                    let res = spi.on_play().await;
                    let _ = tx.unwrap().send(EventRes::OnPlay(res));
                }
                Event::OffPlay(spi) => {
                    let _ = spi.off_play().await;

                    // let stream_id = spi.get_base_stream_info().get_stream_id();
                    // cache::remove_user(stream_id, spi.get_remote_addr());
                }
                Event::EndRecord(_) => {}
            }
        }
    }
}
