use axum::Json;
use axum::response::{IntoResponse, Response};
use common::serde::{Deserialize, Serialize};
use pretend::Response as PretendResponse;
use pretend::client::Bytes;
use common::serde::de::DeserializeOwned;
use common::serde_json;

/// () 被 serde 序列化为一个空对象 {}
/// 传入的是 None，并启用了 skip_serializing_if = "Option::is_none"，那么 data 字段会被完全省略。
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct Resp<T> {
    code: u16,
    msg: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

impl<T> Resp<T> {
    pub fn define(code: u16, msg: impl Into<String>, data: Option<T>) -> Self {
        Self { code, msg: msg.into(), data }
    }
}
//the trait bound `pretend::Response<pretend::client::Bytes>: IntoResponse<Resp<()>>` is not satisfied [E0277]
// Help: the following other types implement trait `IntoResponse<T>`:
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<()>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<JsonResult<T, E>>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<Vec<u8>>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<pretend::Json<T>>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<pretend::Response<()>>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<pretend::Response<JsonResult<T, E>>>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<pretend::Response<Vec<u8>>>`
// `pretend::Response<pretend::client::Bytes>` implements `IntoResponse<pretend::Response<pretend::Json<T>>>`
// and 2 others

// impl<T> From<PretendResponse<Bytes>> for Resp<T>
// where
//     T: DeserializeOwned,
// {
//     fn from(resp: PretendResponse<Bytes>) -> Self {
//         let body = resp.body();
// 
//         // 如果解析失败则返回错误结构
//         serde_json::from_slice::<Resp<T>>(body).unwrap_or_else(|err| {
//             Resp::define(500, format!("response parse error: {}", err), None)
//         })
//     }
// }
// impl<T> IntoResponse for PretendResponse<Bytes>
// where
//     T: DeserializeOwned + Serialize,
// {
//     fn into_response(self) -> Response {
//         let resp: Resp<T> = self.into();
//         Json(resp).into_response()
//     }
// }
// pub struct HttpResult<T>(pub Resp<T>);
// 
// impl<T: Serialize> IntoResponse for HttpResult<T> {
//     fn into_response(self) -> Response {
//         Json(self.0).into_response()
//     }
// }
// impl<T> IntoResponse<Resp<T>> for Response<Bytes>
// where
//     T: for<'de> serde::Deserialize<'de>,
// {
//     fn into_response(self) -> Result<Resp<T>, Box<dyn Error + Send + Sync>> {
//         let body = String::from_utf8(self.body().to_vec())?;
//         let value: Resp<T> = serde_json::from_str(&body)?;
//         Ok(value)
//     }
// }
impl<T: Serialize> IntoResponse for Resp<T> {
    fn into_response(self) -> Response {
        Json(self).into_response()
    }
}

impl<T: Serialize> Resp<T> {
    pub fn build_success() -> Self {
        Self { code: 200, msg: "success".to_string(), data: None }
    }

    pub fn build_failed() -> Self {
        Self { code: 500, msg: "failed".to_string(), data: None }
    }

    pub fn build_failed_by_msg(msg: impl Into<String>) -> Self {
        Self { code: 500, msg: msg.into(), data: None }
    }

    pub fn build_success_data(data: T) -> Self {
        Self { code: 200, msg: "success".to_string(), data: Some(data) }
    }
}