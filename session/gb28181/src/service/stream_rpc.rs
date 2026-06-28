use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use base::serde::{Serialize, de::DeserializeOwned};
use base::serde_json;
use gmv_domain::info::media_info::MediaConfig;
use gmv_domain::info::media_info_ext::MediaMap;
use gmv_domain::info::obj::{
    StreamInfoQo, StreamKey, StreamRecordInfo, TalkAnswerReq, TalkCloseReq, TalkOpenReq,
    TalkOpenResp,
};
use gmv_protocol::common::v1::ErrorDetail;
use gmv_protocol::stream::v1::{
    StreamBoolResponse, StreamJsonRequest, StreamJsonResponse, StreamUnitResponse,
    stream_control_client::StreamControlClient,
};
use std::time::Duration;

use tonic::transport::Channel;

use crate::state::StreamNode;

async fn client(node: &StreamNode) -> GlobalResult<StreamControlClient<Channel>> {
    if node.control_grpc_uri.trim().is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "stream control_grpc_uri is required",
            |msg| error!("{msg}: node={}", node.name),
        ));
    }
    let mut config = base_rpc::RpcChannelConfig::new(node.control_grpc_uri.clone());
    if node.control_grpc_uri.starts_with("https://") {
        config.tls = Some(base_rpc::RpcClientTlsConfig {
            domain_name: url::Url::parse(&node.control_grpc_uri)
                .ok()
                .and_then(|url| url.host_str().map(ToString::to_string)),
            ca_certificate_pem: None,
            client_certificate_pem: None,
            client_private_key_pem: None,
            use_native_roots: true,
            handshake_timeout: Duration::from_secs(5),
        });
    }
    base_rpc::connect_channel(&config)
        .await
        .map(StreamControlClient::new)
        .map_err(|err| {
            GlobalError::new_biz_error(
                BaseErrorCode::Network.code(),
                "connect stream control rpc failed",
                |msg| {
                    error!(
                        "{msg}: node={}, endpoint={}, err={err:?}",
                        node.name, node.control_grpc_uri
                    )
                },
            )
        })
}

fn request<T: Serialize>(value: &T) -> GlobalResult<StreamJsonRequest> {
    Ok(StreamJsonRequest {
        payload_json: serde_json::to_vec(value).hand_log(|msg| error!("{msg}"))?,
    })
}

fn ensure_unit(response: StreamUnitResponse, action: &str) -> GlobalResult<()> {
    match response.error {
        None => Ok(()),
        Some(error) => Err(error_detail(error, action)),
    }
}

fn ensure_bool(response: StreamBoolResponse, action: &str) -> GlobalResult<bool> {
    match response.error {
        None => Ok(response.value),
        Some(error) => Err(error_detail(error, action)),
    }
}

fn decode_json<T: DeserializeOwned>(response: StreamJsonResponse, action: &str) -> GlobalResult<T> {
    if let Some(error) = response.error {
        return Err(error_detail(error, action));
    }
    serde_json::from_slice(&response.payload_json).map_err(|err| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "decode stream rpc response failed",
            |msg| error!("{msg}: action={action}, err={err:?}"),
        )
    })
}

fn error_detail(error: ErrorDetail, action: &str) -> GlobalError {
    let message = if error.message.is_empty() {
        error.code.as_str()
    } else {
        error.message.as_str()
    };
    GlobalError::new_biz_error(BaseErrorCode::Internal.code(), message, |msg| {
        error!("stream rpc {action} failed: {msg}; code={}", error.code)
    })
}

pub async fn init_media(node: &StreamNode, value: &MediaConfig) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let response = client
        .init_media(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_unit(response, "init_media")
}

pub async fn init_media_ext(node: &StreamNode, value: &MediaMap) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let response = client
        .init_media_ext(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_unit(response, "init_media_ext")
}

pub async fn stream_online(node: &StreamNode, value: &StreamKey) -> GlobalResult<bool> {
    let mut client = client(node).await?;
    let response = client
        .stream_online(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_bool(response, "stream_online")
}

pub async fn record_info(
    node: &StreamNode,
    value: &StreamInfoQo,
) -> GlobalResult<StreamRecordInfo> {
    let mut client = client(node).await?;
    let response = client
        .record_info(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    decode_json(response, "record_info")
}

pub async fn close_output(node: &StreamNode, value: &StreamInfoQo) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let response = client
        .close_output_by_ssrc(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_unit(response, "close_output")
}

pub async fn talk_open(node: &StreamNode, value: &TalkOpenReq) -> GlobalResult<TalkOpenResp> {
    let mut client = client(node).await?;
    let response = client
        .talk_open(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    decode_json(response, "talk_open")
}

pub async fn talk_answer(node: &StreamNode, value: &TalkAnswerReq) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let response = client
        .talk_answer(request(value)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_unit(response, "talk_answer")
}

pub async fn talk_close(node: &StreamNode, talk_id: &str) -> GlobalResult<()> {
    let request = TalkCloseReq {
        talk_id: talk_id.to_string(),
    };
    let mut client = client(node).await?;
    let response = client
        .talk_close(self::request(&request)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_unit(response, "talk_close")
}

pub async fn talk_online(node: &StreamNode, talk_id: &str) -> GlobalResult<bool> {
    let request = TalkCloseReq {
        talk_id: talk_id.to_string(),
    };
    let mut client = client(node).await?;
    let response = client
        .talk_online(self::request(&request)?)
        .await
        .hand_log(|msg| error!("{msg}"))?
        .into_inner();
    ensure_bool(response, "talk_online")
}
