use std::net::Ipv4Addr;
use base::exception::{GlobalResult, GlobalResultExt};
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::{pretend, Pretend, Result};
use pretend::{Json, Url};
use shared::info::media_info::MediaStreamConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{StreamKey, StreamRecordInfo};
use shared::info::res::Resp;
use std::str::FromStr;
use std::time::Duration;
use crate::state::model::{AlarmInfo, SingleParam};

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
    pub fn template_ip_port(local_ip: &String, local_port: u16) -> GlobalResult<Pretend<pretend_reqwest::Client, UrlResolver, NoopRequestInterceptor>> {
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
    #[request(method = "POST", path = "/stream/online")]
    async fn stream_online(&self, json: &StreamKey) -> Result<Json<Resp<bool>>>;
    #[request(method = "POST", path = "/record/info")]
    async fn record_info(&self, json: &SingleParam<String>) -> Result<Json<Resp<StreamRecordInfo>>>;
}

#[pretend]
pub trait HttpBiz {
    #[request(method = "POST", path = "")]
    async fn call_alarm_info(&self, json: &AlarmInfo) -> Result<Json<Resp<bool>>>;
}

#[cfg(test)]
mod tests {
    use base::{serde_json, tokio};
    use shared::info::media_info::MediaStreamConfig;
    use crate::http::client::{HttpClient, HttpStream};

    // #[tokio::test]
    async fn test() {
        let info = "{\"ssrc\":9488,\"stream_id\":\"q2qcqqz3Uqqe4qhqqs7Fqqtqqu5Sv6Z5E5t1dK\",\"expires\":null,\"converter\":{\"codec\":null,\"muxer\":{\"flv\":null,\"mp4\":null,\"ts\":null,\"rtp_frame\":null,\"rtp_ps\":null,\"rtp_enc\":null,\"frame\":null},\"filter\":{\"capture\":null}},\"output\":{\"local\":null,\"rtmp\":null,\"dash\":null,\"http_flv\":{},\"hls\":null,\"rtsp\":null,\"gb28181\":null,\"web_rtc\":null}}";
        let ip = "127.0.0.1".to_string();
        let port = 18570;
        let p = HttpClient::template_ip_port(&ip, port).unwrap();
        let info: MediaStreamConfig = serde_json::from_str(info).unwrap();
        let res = p.stream_init(&info).await;
        println!("{:?}", res);
    }
}