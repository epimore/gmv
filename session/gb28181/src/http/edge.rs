use std::collections::HashMap;

use crate::http::res_by_error;
use crate::service::edge_serv;
use crate::utils::edge_token;
use axum::body::Bytes;
use axum::extract::{FromRequest, Multipart, Path, Query, Request};
use axum::http::{HeaderMap, header::CONTENT_TYPE};
use axum::{Json, Router, routing::post};
use base::err::{BaseErrorCode, CodeOutErr};
use base::exception::{GlobalError, GlobalResult};
use base::log::{error, info};
use gmv_domain::info::res::Resp;

pub const UPLOAD_PICTURE: &str = "/upload/picture/{token}";

pub fn routes() -> Router {
    Router::new().route(UPLOAD_PICTURE, post(upload_picture))
}

/// 设备抓拍图片上传入口。
async fn upload_picture(
    Path(token): Path<String>,
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
    request: Request,
) -> Json<Resp<String>> {
    info!("upload_picture: token = {:?}", &token);
    match upload_picture_inner(token, headers, params, request).await {
        Ok(data) => Json(Resp::build_success_data(data)),
        Err(err) => Json(Resp::build_failed_code(
            match &err {
                GlobalError::BizErr(err) => err.code,
                GlobalError::SysErr(_) => BaseErrorCode::Internal.code(),
            },
            err.out_err().into_owned(),
        )),
    }
}

async fn upload_picture_inner(
    token: String,
    headers: HeaderMap,
    params: HashMap<String, String>,
    request: Request,
) -> GlobalResult<String> {
    let session_id = get_param(&params, "SessionID")?.to_string();
    edge_token::check(&session_id, &token)?;
    if !edge_serv::check_pic_token(&token) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Unauthorized.code(),
            "Invalid token",
            |msg| error!("{msg}"),
        ));
    }
    let file_id = params
        .iter()
        .find(|(key, _)| key.to_ascii_lowercase().ends_with("fileid"))
        .map(|(_, value)| value.to_string());
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    if content_type.starts_with("multipart/form-data") {
        handle_multipart_upload(request, &token, &session_id, file_id.as_deref()).await
    } else if content_type.starts_with("image/") {
        handle_binary_upload(
            request,
            &token,
            &session_id,
            file_id.as_deref(),
            &content_type,
        )
        .await
    } else {
        Err(GlobalError::new_biz_error(
            BaseErrorCode::Unsupported.code(),
            "Unsupported Content-Type. Use multipart/form-data or image/*",
            |msg| error!("{msg}"),
        ))
    }
}

fn get_param<'a>(params: &'a HashMap<String, String>, key: &str) -> GlobalResult<&'a str> {
    params.get(key).map(|value| value.as_str()).ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            &format!("Missing `{key}`"),
            |msg| error!("{msg}"),
        )
    })
}

async fn handle_multipart_upload(
    request: Request,
    token: &str,
    session_id: &str,
    file_id: Option<&str>,
) -> GlobalResult<String> {
    let mut multipart = Multipart::from_request(request, &())
        .await
        .map_err(|error| {
            GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                &format!("Failed to parse multipart: {error}"),
                |msg| error!("{msg}"),
            )
        })?;

    while let Some(field) = multipart.next_field().await.map_err(|error| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            &format!("Failed to get next field: {error}"),
            |msg| error!("{msg}"),
        )
    })? {
        let content_type = field.content_type().unwrap_or_default().to_string();
        let has_filename = field.file_name().is_some();
        if content_type.starts_with("image/") && has_filename {
            let bytes = field.bytes().await.map_err(|error| {
                GlobalError::new_biz_error(
                    BaseErrorCode::InvalidRequest.code(),
                    &format!("Failed to get field bytes: {error}"),
                    |msg| error!("{msg}"),
                )
            })?;
            edge_serv::upload(bytes, &content_type, session_id, file_id).await?;
            edge_serv::refresh_pic_upload(token, session_id);
            return Ok("File uploaded successfully as form-data".to_string());
        }
    }

    Err(GlobalError::new_biz_error(
        BaseErrorCode::InvalidRequest.code(),
        "No valid image file found in multipart form",
        |msg| error!("{msg}"),
    ))
}

async fn handle_binary_upload(
    request: Request,
    token: &str,
    session_id: &str,
    file_id: Option<&str>,
    content_type: &str,
) -> GlobalResult<String> {
    let bytes = Bytes::from_request(request, &()).await.map_err(|error| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            &format!("Failed to get request body: {error}"),
            |msg| error!("{msg}"),
        )
    })?;
    edge_serv::upload(bytes, content_type, session_id, file_id).await?;
    edge_serv::refresh_pic_upload(token, session_id);
    Ok("File uploaded successfully as binary".to_string())
}
