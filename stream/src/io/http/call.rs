use crate::state::register::{DEFAULT_EXPIRES, Register};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::info;
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::{Json, Url};
use pretend::{Pretend, Result, pretend};
use shared::info::obj::{
    BaseStreamInfo, InTimeoutEventRes, OutputEventRes, OutputStreamInfo, RegisterStreamInfo,
    StreamPlayInfo, StreamRecordInfo, StreamState,
};
use shared::info::res::Resp;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub struct HttpClient;
pub type HttpTemplate = Arc<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>>;
static HTTP: OnceLock<HttpTemplate> = OnceLock::new();
impl HttpClient {
    fn init(url: &str) -> GlobalResult<HttpTemplate> {
        let url = Url::from_str(url).hand_log(|msg| info!("{msg}"))?;
        let client = pretend_reqwest::reqwest::Client::builder()
            .timeout(DEFAULT_EXPIRES)
            .build()
            .hand_log(|msg| info!("{msg}"))?;
        let pretend =
            pretend::Pretend::for_client(pretend_reqwest::Client::new(client)).with_url(url);
        Ok(Arc::new(pretend))
    }
    pub fn template() -> GlobalResult<HttpTemplate> {
        if let Some(c) = HTTP.get() {
            return Ok(c.clone());
        }
        let client = Self::init(&Register::get_server_conf().hook_uri)?;
        let _ = HTTP.set(client.clone());
        Ok(client)
    }
}

#[pretend]
pub trait HttpSession {
    #[request(method = "POST", path = "/hook/stream/register")]
    async fn stream_register(&self, json: &RegisterStreamInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/hook/stream/input/timeout")]
    async fn stream_input_timeout(
        &self,
        json: &StreamState,
    ) -> Result<Json<Resp<InTimeoutEventRes>>>;
    #[request(method = "POST", path = "/hook/on/play")]
    async fn on_play(&self, json: &StreamPlayInfo) -> Result<Json<Resp<bool>>>;
    #[request(method = "POST", path = "/hook/stream/idle")]
    async fn stream_idle(&self, json: &OutputStreamInfo) -> Result<Json<Resp<OutputEventRes>>>;
    #[request(method = "POST", path = "/hook/off/play")]
    async fn off_play(&self, json: &StreamPlayInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/hook/end/record")]
    async fn end_record(&self, json: &StreamRecordInfo) -> Result<Json<Resp<()>>>;
}
