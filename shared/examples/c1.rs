use std::str::FromStr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use pretend::{pretend, Json, Pretend, Result, Url};
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use common::exception::{GlobalResult, GlobalResultExt};
use common::serde::{Deserialize, Serialize};
use common::{logger, tokio};
use shared::info::res::Resp;
/*
use std::sync::atomic::{AtomicU64, Ordering};

pub const JSON_RPC: &str = "2.0";
static ID: AtomicU64 = AtomicU64::new(1);

fn get_global_id() -> u64 {
    let current = ID.fetch_add(1, Ordering::Relaxed);
    if current == u64::MAX {
        ID.store(1, Ordering::Relaxed);
        1
    } else {
        current
    }
}
*/
pub struct HttpClient;
static HTTP: OnceLock<GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>>> = OnceLock::new();
impl HttpClient {
    fn init(url: &str) -> GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
        let url = Url::from_str(url).hand_log(|msg| println!("{}", msg))?;
        let pretend_client =pretend_reqwest::reqwest::Client::builder().timeout(Duration::from_secs(5)).build().unwrap();
        let pretend = pretend::Pretend::for_client(pretend_reqwest::Client::new(pretend_client))
            .with_url(url);
        Ok(pretend)
    }
    pub fn template() -> &'static GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
        let pretend = HTTP.get_or_init(|| {
            HttpClient::init("http://127.0.0.1:30000")
        });
        pretend
    }
}
#[pretend]
pub trait HttpSession {
    #[request(method = "POST", path = "/put/user")]
    async fn stream_in(&self, json: &User) -> Result<Json<Resp<()>>>;
}


#[derive(Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct User {
    id: u64,
    name: String,
}

#[tokio::main]
async fn main() {
    let user = User {
        id: 1,
        name: "zhangsan".to_string(),
    };
    let pretend = match HttpClient::template() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Pretend client init failed: {:?}", e);
            return;
        }
    };
    let resp = pretend.stream_in(&user).await;
    println!("resp: {:#?}", resp);
}