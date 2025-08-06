use axum::{
    extract::{FromRequest, Multipart, Query, Request},
    http::HeaderMap,
    response::IntoResponse,
    routing::post,
    RequestExt, Router,
};
use std::collections::HashMap;

use common::{bytes::Bytes, log::error};

use crate::{http::UPLOAD_PICTURE, service::biz, utils::edge_token};

pub fn routes() -> Router {
    Router::new().route(UPLOAD_PICTURE, post(upload_picture))
}

async fn upload_picture(
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    req: Request,
) -> Result<impl IntoResponse, String> {
    let session_id = get_param(&params, "SessionID")?;
    let token = get_param(&params, "token")?;
    edge_token::check(session_id, token).map_err(|_| "Invalid token".to_string())?;

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
            handle_multipart_upload(req, params).await
        }
        ct if ct.starts_with("image/") => {
            handle_binary_upload(req, params).await
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
    params: HashMap<String, String>,
) -> Result<impl IntoResponse, String> {
    let mut multipart = Multipart::from_request(req, &Default::default())
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
            biz::upload(data, params)
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
    params: HashMap<String, String>,
) -> Result<impl IntoResponse, String> {
    let data = Bytes::from_request(req, &Default::default())
        .await
        .map_err(|e| {
            error!("Failed to get request body: {}", e);
            format!("Failed to get request body: {}", e)
        })?;

    biz::upload(data, params)
        .await
        .map_err(|e| format!("Failed to upload file: {}", e))?;

    Ok("File uploaded successfully as binary")
}
