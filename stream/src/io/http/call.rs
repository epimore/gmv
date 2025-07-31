// use std::sync::{Arc, OnceLock};
// use std::time::Duration;
// use feign::{client, ClientResult};
// use shared::info::res::Resp;
// use crate::biz::call::BaseStreamInfo;
// use crate::state::{cache, TIME_OUT};
// 
// async fn client_builder() -> ClientResult<reqwest::Client> {
//     Ok(reqwest::ClientBuilder::new().timeout(Duration::from_millis(TIME_OUT)).build()?)
// }
// impl HttpSession {
//     pub fn template() -> &'static Self {
//         static HTTP: OnceLock<HttpSession> = OnceLock::new();
//         HTTP.get_or_init(|| HttpSession::builder()
//             .set_host_arc(Arc::new(cache::get_server_conf().get_hook_uri()))
//             .build())
//     }
// }
// #[client(
//     path = "",
//     client_builder = "client_builder"
// )]
// pub trait HttpSession {
//     #[post(path = "/stream/register")]
//     async fn stream_register(&self, #[json] info: &BaseStreamInfo) -> anyhow::Result<Resp<()>>;
// }

use crate::biz::call::BaseStreamInfo;
use crate::state::{cache, TIME_OUT};
use common::exception::{GlobalResult, GlobalResultExt};
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::Url;
use pretend::{pretend, Pretend, Result};
use shared::info::res::Resp;
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;

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
    async fn stream_register(&self, json: &BaseStreamInfo) -> Result<Resp<()>>;
    #[request(method = "POST", path = "/stream/idle")]
    async fn stream_idle(&self, json: &BaseStreamInfo) -> Result<Resp<u8>>;
}