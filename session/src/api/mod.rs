use poem::FromRequest;
use poem_openapi::OpenApi;
use poem_openapi::payload::{Form, Json};
use crate::general::model::{PlayLiveModel, ResultMessageData, StreamInfo};

pub struct RestApi;

#[OpenApi(prefix_path = "/api")]
impl RestApi {
    #[allow(non_snake_case)]
    #[oai(path = "/play/live/stream", method = "post")]
    /// 点播监控实时画面 transMode 默认0 udp 模式, 1 tcp 被动模式,2 tcp 主动模式， 目前只支持 0
    async fn play_live(&self, live: Form<PlayLiveModel>) -> Json<ResultMessageData<Option<StreamInfo>>> {

        // match handler::play_live(&deviceId.0, &channelId.0, 0, "twoLevel").await {
        //     Err(err) => {
        //         error!("点播失败；{}",err);
        //         Json(ResultMessageData::build_failure())
        //     }
        //     Ok(info) => { Json(ResultMessageData::build_success(Some(info))) }
        // }
        Json(ResultMessageData::build_success_none())
    }
}

pub struct HookApi;

#[OpenApi(prefix_path = "/hook")]
impl HookApi {
    #[oai(path = "/test", method = "get")]
    async fn test(&self) {}
}