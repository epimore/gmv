use crate::register::core::DEFAULT_EXPIRES;
use crate::state::model::AlarmInfo;
use base::dashmap;
use base::dashmap::DashMap;
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::info;
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::{Json, Url};
use pretend::{Pretend, Result, pretend};
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{
    StreamInfoQo, StreamKey, StreamRecordInfo, TalkAnswerReq, TalkCloseReq, TalkOpenReq,
    TalkOpenResp,
};
use shared::info::res::Resp;
use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

type HttpTemplate = Arc<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>>;
static CLIENT_POOL: OnceLock<DashMap<String, HttpTemplate>> = OnceLock::new();
pub struct HttpClient;
impl HttpClient {
    fn pool() -> &'static DashMap<String, HttpTemplate> {
        CLIENT_POOL.get_or_init(|| DashMap::new())
    }
    fn init(url: &str) -> GlobalResult<HttpTemplate> {
        let url_parsed = Url::parse(url).hand_log(|msg| info!("{msg}"))?;

        let client = pretend_reqwest::reqwest::Client::builder()
            .timeout(DEFAULT_EXPIRES)
            .build()
            .hand_log(|msg| info!("{msg}"))?;

        let pretend =
            pretend::Pretend::for_client(pretend_reqwest::Client::new(client)).with_url(url_parsed);

        Ok(Arc::new(pretend))
    }

    pub fn template(url: &str) -> GlobalResult<HttpTemplate> {
        let pool = Self::pool();
        if let Some(c) = pool.get(url) {
            return Ok(c.clone());
        }
        let client = Self::init(url)?;
        // 双检 + 并发安全
        match pool.entry(url.to_string()) {
            dashmap::mapref::entry::Entry::Occupied(e) => Ok(e.get().clone()),
            dashmap::mapref::entry::Entry::Vacant(e) => {
                e.insert(client.clone());
                Ok(client)
            }
        }
    }
    pub fn template_ip_port(local_ip: &String, local_port: u16) -> GlobalResult<HttpTemplate> {
        let url = format!("http://{}:{}", local_ip, local_port);
        Self::template(&url)
    }
}

#[pretend]
pub trait HttpStream {
    #[request(method = "POST", path = "/listen/media")]
    async fn stream_init(&self, json: &MediaConfig) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/sdp/media")]
    async fn stream_init_ext(&self, json: &MediaMap) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/stream/online")]
    async fn stream_online(&self, json: &StreamKey) -> Result<Json<Resp<bool>>>;
    #[request(method = "POST", path = "/record/info")]
    async fn record_info(&self, json: &StreamInfoQo) -> Result<Json<Resp<StreamRecordInfo>>>;
    #[request(method = "POST", path = "/close/output")]
    async fn close_output(&self, json: &StreamInfoQo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/talk/open")]
    async fn talk_open(&self, json: &TalkOpenReq) -> Result<Json<Resp<TalkOpenResp>>>;
    #[request(method = "POST", path = "/talk/answer")]
    async fn talk_answer(&self, json: &TalkAnswerReq) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/talk/close")]
    async fn talk_close(&self, json: &TalkCloseReq) -> Result<Json<Resp<()>>>;
}

#[pretend]
pub trait HttpBiz {
    #[request(method = "POST", path = "")]
    async fn call_alarm_info(&self, json: &AlarmInfo) -> Result<Json<Resp<bool>>>;
}
