use crate::state::{cache, TIME_OUT};
use common::exception::{GlobalResult, GlobalResultExt};
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::{Json, Url};
use pretend::{pretend, Pretend, Result};
use shared::info::res::Resp;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;
use shared::info::obj::{BaseStreamInfo, StreamPlayInfo, StreamRecordInfo, StreamState};

pub struct HttpClient;
static HTTP: OnceLock<GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>>> = OnceLock::new();
impl HttpClient {
    fn init(url: &str) -> GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
        let url = Url::from_str(url).hand_log(|msg| println!("{}", msg))?;
        let client = pretend_reqwest::reqwest::Client::builder().timeout(Duration::from_millis(TIME_OUT)).build().unwrap();
        let pretend = pretend::Pretend::for_client(pretend_reqwest::Client::new(client))
            .with_url(url);
        Ok(pretend)
    }
    pub fn template() -> &'static GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
        let pretend = HTTP.get_or_init(|| {
            HttpClient::init(cache::get_server_conf().get_hook_uri())
        });
        pretend
    }
}

#[pretend]
pub trait HttpSession {
    #[request(method = "POST", path = "/stream/register")]
    async fn stream_register(&self, json: &BaseStreamInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/stream/input/timeout")]
    async fn stream_input_timeout(&self, json: &StreamState) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/on/play")]
    async fn on_play(&self, json: &StreamPlayInfo) -> Result<Json<Resp<bool>>>;
    #[request(method = "POST", path = "/stream/idle")]
    async fn stream_idle(&self, json: &BaseStreamInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/off/play")]
    async fn off_play(&self, json: &StreamPlayInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/end/record")]
    async fn end_record(&self, json: &StreamRecordInfo) -> Result<Json<Resp<()>>>;
}