use base::serde::{Deserialize, Serialize};

/// () 被 serde 序列化为一个空对象 {}
/// 传入的是 None，并启用了 skip_serializing_if = "Option::is_none"，那么 data 字段会被完全省略。
#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "base::serde")]
pub struct Resp<T> {
    pub code: u16,
    pub msg: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<T>,
}

impl<T> Resp<T> {
    pub fn define(code: u16, msg: impl Into<String>, data: Option<T>) -> Self {
        Self { code, msg: msg.into(), data }
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