use log::info;
use poem::FromRequest;
use poem_openapi::OpenApi;
use poem_openapi::payload::{Form, Json};
use crate::general::model::{PlayLiveModel, ResultMessageData, StreamInfo};
use crate::service::{BaseStreamInfo, handler, StreamPlayInfo, StreamState};


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
    #[oai(path = "/stream/in", method = "post")]
    async fn stream_in(&self, base_stream_info: Json<BaseStreamInfo>) -> Json<ResultMessageData<bool>> {
        let info = base_stream_info.0;
        info!("stream_in = {:?}", &info);
        handler::stream_in(info).await;
        Json(ResultMessageData::build_success_none())
    }
    #[oai(path = "/stream/input/timeout", method = "post")]
    async fn stream_input_timeout(&self, stream_state: Json<StreamState>) -> Json<ResultMessageData<bool>> {
        let info = stream_state.0;
        info!("stream_input_timeout = {:?}", &info);
        handler::stream_input_timeout(info);
        Json(ResultMessageData::build_success_none())
    }
    #[oai(path = "/on/play", method = "post")]
    async fn on_play(&self, stream_play_info: Json<StreamPlayInfo>) -> Json<ResultMessageData<bool>> {
        let info = stream_play_info.0;
        info!("on_play = {:?}", &info);
        Json(ResultMessageData::build_success(handler::on_play(info)))
    }
    #[oai(path = "/off/play", method = "post")]
    async fn off_play(&self, stream_play_info: Json<StreamPlayInfo>) -> Json<ResultMessageData<bool>> {
        let info = stream_play_info.0;
        info!("off_play = {:?}", &info);
        Json(ResultMessageData::build_success(handler::off_play(info)))
    }
}