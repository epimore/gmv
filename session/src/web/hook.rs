use common::log::info;
use poem_openapi::OpenApi;
use poem_openapi::payload::{Json};
use crate::general::model::{ResultMessageData};
use crate::service::{BaseStreamInfo, handler, StreamPlayInfo, StreamState, StreamRecordInfo};


////callback uri start
// //ssrc流注册
// pub const STREAM_IN: &str = "/stream/in";
// //ssrc流无操作
// pub const STREAM_IDLE: &str = "/stream/idle";
// //播放流
// pub const ON_PLAY: &str = "/on/play";
// //关闭播放
// pub const OFF_PLAY: &str = "/off/play";
// //录制结束
// pub const END_RECORD: &str = "/end/record";
// //等待流超时
// pub const STREAM_INPUT_TIMEOUT: &str = "/stream/input/timeout";
pub struct HookApi;

#[OpenApi(prefix_path = "/hook")]
impl HookApi {
    ///流媒体监听ssrc：接收到流注册事件；信令回调/api/play/xxx返回播放流信息
    #[oai(path = "/stream/register", method = "post")]
    async fn stream_in(&self, base_stream_info: Json<BaseStreamInfo>) -> Json<ResultMessageData<bool>> {
        let info = base_stream_info.0;
        info!("stream_in = {:?}", &info);
        handler::stream_in(info).await;
        Json(ResultMessageData::build_success_none())
    }
    ///流媒体监听ssrc：等待流8秒，超时未接收到；发送一次接收流超时事件；信令下发设备取消推流，并清理缓存会话；
    /// 【流注册等待超时，信令回调/api/play/xxx返回响应超时信息】
    #[oai(path = "/stream/input/timeout", method = "post")]
    async fn stream_input_timeout(&self, stream_state: Json<StreamState>) -> Json<ResultMessageData<bool>> {
        let info = stream_state.0;
        info!("stream_input_timeout = {:?}", &info);
        handler::stream_input_timeout(info);
        Json(ResultMessageData::build_success_none())
    }
    ///流媒体监测到用户点播流：发送一次用户点播流事件，用于鉴权
    #[oai(path = "/on/play", method = "post")]
    async fn on_play(&self, stream_play_info: Json<StreamPlayInfo>) -> Json<ResultMessageData<bool>> {
        let info = stream_play_info.0;
        info!("on_play = {:?}", &info);
        Json(ResultMessageData::build_success(handler::on_play(info)))
    }
    ///流媒体监测到用户断开点播流：发送一次用户关闭流事件：
    #[oai(path = "/off/play", method = "post")]
    async fn off_play(&self, stream_play_info: Json<StreamPlayInfo>) -> Json<ResultMessageData<bool>> {
        let info = stream_play_info.0;
        info!("off_play = {:?}", &info);
        handler::off_play(info).await;
        Json(ResultMessageData::build_success_none())
    }
    ///流媒体监测到无人连接媒体流：发送一次流空闲事件【配置为不关闭流，则不发送】：信令下发设备关闭推流，并清理缓存会话
    #[oai(path = "/stream/idle", method = "post")]
    async fn stream_idle(&self, stream_play_info: Json<BaseStreamInfo>) -> Json<ResultMessageData<bool>> {
        let info = stream_play_info.0;
        info!("stream_idle = {:?}", &info);
        handler::stream_idle(info).await;
        Json(ResultMessageData::build_success_none())
    }

    ///完成录像
    #[oai(path = "/end/record", method = "post")]
    async fn end_record(&self, stream_record_info: Json<StreamRecordInfo>) -> Json<ResultMessageData<bool>> {
        let info = stream_record_info.0;
        info!("end_record = {:?}", &info);
        handler::end_record(info).await;
        Json(ResultMessageData::build_success_none())
    }
}