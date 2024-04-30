use common::err::GlobalResult;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::oneshot::Sender;
use crate::biz::call::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};

pub enum Event {
    streamIn(BaseStreamInfo),
    streamIdle(BaseStreamInfo),
    streamTimeout(StreamState),
    streamUnknown(RtpInfo),
    onPlay(StreamPlayInfo),
    offPlay(StreamPlayInfo),
    endRecord(StreamRecordInfo),
}

//None-未响应或响应超时等异常
pub enum EventRes {
    streamIn(Option<bool>),
    //收到国标媒体流事件：响应内容不敏感;some-成功接收;None-未成功接收->查看流是否被使用(观看/录制)->否->调用streamIdle事件
    streamIdle(Option<u8>),
    //国标流闲置事件：响应0,关闭流，1-255为等待时间，单位秒；未响应则取消监听该ssrc
    streamTimeout(Option<bool>),
    //接收国标媒体流超时事件：取消监听该SSRC,响应内容不敏感;
    streamUnknown(Option<bool>),
    //未知ssrc流事件；响应内容不敏感,some-成功接收;None-未成功接收
    onPlay(Option<bool>),
    //用户点播媒体流事件,none与false-回复用户401，true-写入流
    offPlay(Option<bool>),
    //用户关闭媒体流事件，响应内容不敏感;some-成功接收;None-未成功接收
    endRecord(Option<bool>),//录像完成事件：响应内容不敏感;some-成功接收;None-未成功接收->查看流是否被使用(观看)->否->调用streamIdle事件
}

impl Event {
    pub async fn event_loop(mut rx: Receiver<(Event, Sender<EventRes>)>) -> GlobalResult<()> {
        while let Some((event, tx)) = rx.recv().await {
            match event {
                Event::streamIn(_) => {}
                Event::streamIdle(_) => {}
                Event::streamTimeout(_) => {}
                Event::streamUnknown(_) => {}
                Event::onPlay(_) => {}
                Event::offPlay(_) => {}
                Event::endRecord(_) => {}
            }
        }
        unimplemented!()
    }
}
