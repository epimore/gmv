use base::log::{info, warn};
use base::serde::{Serialize, de::DeserializeOwned};
use base::serde_json;
use gmv_protocol::session::v1::{
    SessionHookRequest, SessionHookResponse, session_hook_client::SessionHookClient,
};

pub async fn call_session_hook_rpc<T>(
    endpoint: &str,
    event_type: &str,
    payload: &T,
) -> Option<SessionHookResponse>
where
    T: Serialize + ?Sized,
{
    let endpoint = endpoint.trim();
    if endpoint.is_empty() {
        warn!("session hook rpc endpoint is empty: event_type={event_type}");
        return None;
    }

    let payload_json = match serde_json::to_vec(payload) {
        Ok(payload_json) => payload_json,
        Err(err) => {
            warn!("session hook rpc payload encode failed: event_type={event_type}, err={err:?}");
            return None;
        }
    };
    info!(
        "session hook rpc outbound: endpoint={endpoint}, event_type={event_type}, payload_bytes={}",
        payload_json.len()
    );

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
