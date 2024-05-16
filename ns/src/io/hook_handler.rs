use ffmpeg_next::picture::Type::S;
use common::err::GlobalResult;
use common::tokio::sync::mpsc::Receiver;
use common::tokio::sync::oneshot::Sender;
use crate::biz::call::{BaseStreamInfo, RtpInfo, StreamPlayInfo, StreamRecordInfo, StreamState};
use crate::state::cache;

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
    //收到国标媒体流事件：响应内容不敏感;some-成功接收;None-未成功接收
    streamIn(Option<bool>),
    //用户关闭播放时检测国标流是否闲置事件：响应0,关闭流，1-255为等待时间，单位秒；未响应则取消监听该ssrc
    streamIdle(Option<u8>),
    //接收国标媒体流超时事件：取消监听该SSRC,响应内容不敏感;
    streamTimeout(Option<bool>),
    //未知ssrc流事件；响应内容不敏感,some-成功接收;None-未成功接收
    streamUnknown(Option<bool>),
    //用户点播媒体流事件,none与false-回复用户401，true-写入流
    onPlay(Option<bool>),
    //用户关闭媒体流事件;some-成功接收,some(true)-关闭整个流;None-未成功接收
    offPlay(Option<bool>),
    //录像完成事件：响应内容不敏感;some-成功接收;None-未成功接收->查看流是否被使用(观看)->否->调用streamIdle事件
    endRecord(Option<bool>),
}

impl Event {
    pub async fn event_loop(mut rx: Receiver<(Event, Option<Sender<EventRes>>)>) -> GlobalResult<()> {
        while let Some((event, tx)) = rx.recv().await {
            match event {
                Event::streamIn(bsi) => {
                    bsi.stream_in().await;
                }
                Event::streamIdle(_) => {}
                Event::streamTimeout(ss) => {
                    let _ = ss.stream_input_timeout().await;
                }
                Event::streamUnknown(_) => {}
                Event::onPlay(spi) => {
                    let res = spi.on_play().await;
                    let _ = tx.unwrap().send(EventRes::onPlay(res));
                }
                Event::offPlay(spi) => {
                    if let Some(true) = spi.off_play().await {
                        let stream_id = spi.get_base_stream_info().get_stream_id();
                        cache::remove_by_stream_id(stream_id);
                    }
                }
                Event::endRecord(_) => {}
            }
        }
        unimplemented!()
    }
}
