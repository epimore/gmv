use crate::http::{get_gmv_token, res_by_error};
use crate::service::edge_serv;
use crate::state::model::SnapshotImage;
use axum::{Json, Router, http::HeaderMap, routing::post};
use base::log::info;
use gmv_domain::info::res::Resp;

pub const SNAPSHOT_IMAGE: &str = "/snapshot/image";
pub fn routes() -> Router {
    Router::new().route(SNAPSHOT_IMAGE, post(snapshot_image))
}

/// 采集摄像机当前画面快照；图片上传接收入口已迁移到 guard。
async fn snapshot_image(headers: HeaderMap, Json(info): Json<SnapshotImage>) -> Json<Resp<String>> {
    info!("snapshot_image: body = {:?}", &info);
    match get_gmv_token(headers) {
        Ok(_token) => match edge_serv::snapshot_image(info).await {
            Ok(data) => Json(Resp::build_success_data(data)),
            Err(err) => Json(res_by_error(err)),
        },
        Err(err) => Json(res_by_error(err)),
    }
}
