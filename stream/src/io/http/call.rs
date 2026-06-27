use crate::state::register::{DEFAULT_EXPIRES, Register};
use base::exception::{GlobalResult, GlobalResultExt};
use base::log::{info, warn};
use base::serde::{Serialize, de::DeserializeOwned};
use base::serde_json;
use gmv_domain::info::obj::{
    BaseStreamInfo, InTimeoutEventRes, OutputEventRes, OutputStreamInfo, RegisterStreamInfo,
    StreamPlayInfo, StreamRecordInfo, StreamState, TalkClosedEvent, UnknownStreamEvent,
};
use gmv_domain::info::res::Resp;
use gmv_protocol::session::v1::{
    SessionHookRequest, SessionHookResponse, session_hook_client::SessionHookClient,
};
use pretend::interceptor::NoopRequestInterceptor;
use pretend::resolver::UrlResolver;
use pretend::{Json, Url};
use pretend::{Pretend, Result, pretend};
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

pub async fn try_session_hook_rpc<T>(event_type: &str, payload: &T) -> Option<SessionHookResponse>
where
    T: Serialize + ?Sized,
{
    let endpoint = Register::get_server_conf().hook_rpc_uri.clone();
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        return None;
    }

    let payload_json = match serde_json::to_vec(payload) {
        Ok(payload_json) => payload_json,
        Err(err) => {
            warn!("session hook rpc payload encode failed: event_type={event_type}, err={err:?}");
            return None;
        }
    };

    let mut client = match SessionHookClient::connect(endpoint.to_string()).await {
        Ok(client) => client,
        Err(err) => {
            warn!(
                "session hook rpc connect failed: endpoint={endpoint}, event_type={event_type}, err={err:?}"
            );
            return None;
        }
    };

    match client
        .handle_hook(tonic::Request::new(SessionHookRequest {
            operation: None,
            event_type: event_type.to_string(),
            payload_json,
        }))
        .await
    {
        Ok(response) => Some(response.into_inner()),
        Err(err) => {
            warn!(
                "session hook rpc call failed: endpoint={endpoint}, event_type={event_type}, err={err:?}"
            );
            None
        }
    }
}

pub fn decode_hook_payload<T>(response: &SessionHookResponse) -> Option<T>
where
    T: DeserializeOwned,
{
    match serde_json::from_slice(&response.payload_json) {
        Ok(payload) => Some(payload),
        Err(err) => {
            warn!("session hook rpc response decode failed: err={err:?}, response={response:?}");
            None
        }
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
    #[request(method = "POST", path = "/hook/stream/unknown")]
    async fn stream_unknown(&self, json: &UnknownStreamEvent) -> Result<Json<Resp<bool>>>;
    #[request(method = "POST", path = "/hook/on/play")]
    async fn on_play(&self, json: &StreamPlayInfo) -> Result<Json<Resp<bool>>>;
    #[request(method = "POST", path = "/hook/stream/idle")]
    async fn stream_idle(&self, json: &OutputStreamInfo) -> Result<Json<Resp<OutputEventRes>>>;
    #[request(method = "POST", path = "/hook/off/play")]
    async fn off_play(&self, json: &StreamPlayInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/hook/end/record")]
    async fn end_record(&self, json: &StreamRecordInfo) -> Result<Json<Resp<()>>>;
    #[request(method = "POST", path = "/hook/talk/closed")]
    async fn talk_closed(&self, json: &TalkClosedEvent) -> Result<Json<Resp<bool>>>;
}
