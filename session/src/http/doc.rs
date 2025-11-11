use utoipa::OpenApi;
// 安全配置
use crate::http::api;
use crate::http::hook;
use crate::http::edge;
use crate::state::model::*;
use shared::info::obj::*;
use utoipa::Modify;
use utoipa::openapi::security::ApiKeyValue;

struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        if let Some(components) = openapi.components.as_mut() {
            components.add_security_scheme(
                "gmv_token",
                utoipa::openapi::security::SecurityScheme::ApiKey(
                    utoipa::openapi::security::ApiKey::Header(ApiKeyValue::new("gmv-token")),
                ),
            );
        }
    }
}
// 定义 OpenAPI 文档
#[derive(OpenApi)]
#[openapi(
    paths(
        api::play_living,
        api::play_back,
        api::play_seek,
        api::play_speed,
        api::control_ptz,
        api::download_mp4,
        api::download_stop,
        api::downing_info,
        api::rm_file,
        hook::stream_register,
        hook::stream_input_timeout,
        hook::on_play,
        hook::off_play,
        hook::stream_idle,
        hook::end_record,
        edge::upload_picture
    ),
    components(
        schemas(
            PlayLiveModel,
            PlayBackModel,
            PlaySeekModel,
            PlaySpeedModel,
            PtzControlModel,
            StreamInfo,
            StreamQo,
            StreamRecordInfo,
            BaseStreamInfo,
            StreamPlayInfo,
            StreamState,
        ),
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "设备媒体流操作API", description = "设备媒体流播放和控制相关接口"),
        (name = "流媒体服务回调接口", description = "流媒体服务回调相关接口"),
        (name = "图片采集", description = "图片采集上传相关接口")
    )
)]
struct ApiDoc;

pub fn openapi() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
