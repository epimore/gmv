use axum::{
    extract::{FromRequest, Multipart, Query, Request},
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    Router,
};
use std::collections::HashMap;

use base::{bytes::Bytes, log::error};
use crate::{http::UPLOAD_PICTURE, service::edge_serv, utils::edge_token};

pub fn routes() -> Router {
    Router::new().route(UPLOAD_PICTURE, post(upload_picture))
}

#[utoipa::path(
    post,
    path = "/edge/upload/picture",
    params(
        ("SessionID" = String, Query, description = "Edge 会话 ID", example = "sess_abc123"),
        ("token" = String, Query, description = "认证 token", example = "tkn_xyz789"),
        ("fileId" = String, Query, description = "文件ID", example = "field_id_123132"),
    ),
    request_body(
        content_type = "multipart/form-data",
        content = Object,
        example = json!({
            "description": "支持两种上传方式：",
            "1. multipart/form-data": "表单中包含一个 image 类型的 file 字段",
            "2. raw binary": "直接发送 image/png、image/jpeg 等 image/* 类型的二进制数据"
        })
    ),
    responses(
        (status = 200, description = "上传成功", body = String, example = "File uploaded successfully as form-data"),
        (status = 400, description = "请求参数或格式错误", body = String, example = "Missing `SessionID`"),
        (status = 401, description = "Token 无效", body = String, example = "Invalid token"),
        (status = 500, description = "服务器内部错误", body = String, example = "Failed to upload file: ...")
    ),
    tag = "图片采集"
)]
/// 图片采集上传接收接口
async fn upload_picture(
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    req: Request,
) -> Result<impl IntoResponse, String> {
    let session_id = get_param(&params, "SessionID")?;
    let token = get_param(&params, "token")?;
    edge_token::check(session_id, token).map_err(|_| "Invalid token".to_string())?;
    let file_id_opt = params
        .iter()
        .find(|(key, _)| key.to_lowercase().ends_with("fileid"))
        .map(|(_, value)| value);
    let content_type = headers
        .get("Content-Type")
        .ok_or_else(|| "Missing Content-Type header".to_string())
        .and_then(|value| {
            value
                .to_str()
                .map_err(|_| "Invalid Content-Type header".to_string())
        })?;

    match content_type {
        ct if ct.starts_with("multipart/form-data") => {
            handle_multipart_upload(req, session_id, file_id_opt).await
        }
        ct if ct.starts_with("image/") => {
            handle_binary_upload(req, session_id, file_id_opt).await
        }
        _ => {
            let err = format!(
                "Unsupported Content-Type: {}. Use multipart/form-data or image/*",
                content_type
            );
            error!("{}", err);
            Err(err)
        }
    }
}

/// 提取参数字段
fn get_param<'a>(params: &'a HashMap<String, String>, key: &str) -> Result<&'a str, String> {
    params
        .get(key)
        .map(|s| s.as_str())
        .ok_or_else(|| format!("Missing `{}`", key))
}

/// 处理 multipart/form-data 上传
async fn handle_multipart_upload(
    req: Request,
    session_id: &str,
    file_id_opt: Option<&String>,
) -> Result<&'static str, String> {
    let mut multipart = Multipart::from_request(req, &())
        .await
        .map_err(|e| {
            error!("Failed to parse multipart: {}", e);
            format!("Failed to parse multipart: {}", e)
        })?;

    while let Some(field) = multipart.next_field().await.map_err(|e| {
        error!("Failed to get next field: {}", e);
        format!("Failed to get next field: {}", e)
    })? {
        let is_image = field
            .content_type()
            .map_or(false, |ct| ct.starts_with("image/"));

        let has_filename = field.file_name().is_some();

        if is_image && has_filename {
            let data = field.bytes().await.map_err(|e| {
                error!("Failed to get field bytes: {}", e);
                format!("Failed to get field bytes: {}", e)
            })?;
            edge_serv::upload(data, session_id, file_id_opt)
                .await
                .map_err(|e| format!("Failed to upload file: {}", e))?;
            return Ok("File uploaded successfully as form-data");
        }
    }

    Err("No valid image file found in multipart form".to_string())
}

/// 处理 image/* 二进制上传
async fn handle_binary_upload(
    req: Request,
    session_id: &str,
    file_id_opt: Option<&String>,
) -> Result<&'static str, String> {
    let data = Bytes::from_request(req, &())
        .await
        .map_err(|e| {
            error!("Failed to get request body: {}", e);
            format!("Failed to get request body: {}", e)
        })?;

    edge_serv::upload(data, session_id, file_id_opt)
        .await
        .map_err(|e| format!("Failed to upload file: {}", e))?;

    Ok("File uploaded successfully as binary")
}
