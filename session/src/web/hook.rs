use poem::FromRequest;
use poem_openapi::OpenApi;
use poem_openapi::payload::{Form, Json};
use crate::general::model::{PlayLiveModel, ResultMessageData, StreamInfo};
pub struct HookApi;

#[OpenApi(prefix_path = "/hook")]
impl HookApi {
    #[oai(path = "/test", method = "get")]
    async fn test(&self) {}
}