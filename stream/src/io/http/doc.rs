use utoipa::OpenApi;
use shared::info::obj::*;
use shared::info::media_info_ext::MediaMap;
use shared::info::media_info::MediaConfig;
use crate::io::http::out;
use crate::io::http::api;

// 定义 OpenAPI 文档
#[derive(OpenApi)]
#[openapi(
    paths(
        api::listen_media,
        api::sdp_media,
        api::stream_online,
        api::record_info,
        out::handler,
    ),
    components(
        schemas(
            MediaConfig,
            MediaMap,
            StreamKey,
            StreamInfoQo,
            StreamRecordInfo,
        ),
    ),
    tags(
        (name = "媒体流操作", description = "媒体流操作相关接口"),
        (name = "HTTP播放音视频", description = "HTTP播放音视频接口"),
    )
)]
struct ApiDoc;

pub fn openapi() -> utoipa::openapi::OpenApi {
    ApiDoc::openapi()
}
