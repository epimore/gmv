use std::net::Ipv4Addr;
use common::exception::{GlobalResult, GlobalResultExt};
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::{pretend, Pretend, Result};
use pretend::{Json, Url};
use shared::info::media_info::MediaStreamConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::StreamKey;
use shared::info::res::Resp;
use std::str::FromStr;
use std::time::Duration;

const TIME_OUT: u64 = 8000;
pub struct HttpClient;
impl HttpClient {
    pub fn template(url: &str) -> GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
        let url = Url::from_str(url).hand_log(|msg| println!("{}", msg))?;
        let client = pretend_reqwest::reqwest::Client::builder().timeout(Duration::from_millis(TIME_OUT)).build().unwrap();
        let pretend = pretend::Pretend::for_client(pretend_reqwest::Client::new(client))
            .with_url(url);
        Ok(pretend)
    }
    pub fn template_ip_port(local_ip: String, local_port: u16) -> GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
        let uri = format!("http://{}:{}", local_ip, local_port);
        Self::template(&uri)
    }
}

// struct TokenInterceptor {
//     app_id: String,
//     token: String,
// }
// 
// impl InterceptRequest for TokenInterceptor {
//     fn intercept(&self, mut request: Request) -> Result<Request> {
//         let value = HeaderValue::from_str(&self.token)
//             .map_err(|e| {
//                 warn!("{}", e);
//                 Error::client(e)
//             })?;
//         request.headers.append("Gmv-Token", value);
//         Ok(request)
//     }
// }

#[pretend]
pub trait HttpStream {
    #[request(method = "POST", path = "/listen/ssrc")]
    async fn stream_init(&self, json: &MediaStreamConfig) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/rtp/media")]
    async fn stream_init_ext(&self, json: &MediaMap) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/stream/living")]
    async fn stream_living(&self, json: &StreamKey) -> Result<Json<Resp<bool>>>;
}

// #[pretend]
// pub trait HttpBiz {
// async fn call_alarm_info(&self, json: &AlarmInfo) -> GlobalResult<bool>
// }