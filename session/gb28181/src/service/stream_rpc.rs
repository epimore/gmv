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
use gmv_nodec::error as node_error;
use gmv_protocol::common::v1::{EndpointMode, ErrorDetail, OperationRef};
use gmv_protocol::stream::v1::{
    CreateOutputRequest, OutputInfo, QueryStreamRequest, QueryStreamResponse,
    ReleaseSubscriptionOutputsRequest, StopReceivePhase, StopReceiveRequest, StopReceiveResponse,
    StreamBoolResponse, StreamJsonRequest, StreamJsonResponse, StreamState, StreamUnitResponse,
    stream_control_client::StreamControlClient,
};
use std::time::{Duration, Instant};

use tonic::transport::Channel;

use crate::state::StreamNode;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StreamInputObservation {
    pub ssrc: u32,
    pub lifecycle_generation: u64,
    pub last_packet_at_ms: u64,
    pub packet_count: u64,
    pub input_idle_timeout_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FinalizeReceiveResult {
    Finalized(StreamInputObservation),
    InputChanged(StreamInputObservation),
}

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
    let started = Instant::now();
    base::log::debug!(
        "session rpc client outbound: service=stream_control, node={}, endpoint={}",
        node.name,
        node.control_grpc_uri
    );
    base_rpc::connect_channel(&config)
        .await
        .map(|channel| {
            base::log::debug!(
                "session rpc client inbound: service=stream_control, node={}, endpoint={}, status=ok, elapsed_ms={}",
                node.name,
                node.control_grpc_uri,
                started.elapsed().as_millis()
            );
            StreamControlClient::new(channel)
        })
        .map_err(|err| {
            base::log::debug!(
                "session rpc client inbound: service=stream_control, node={}, endpoint={}, status=error, elapsed_ms={}, err={err:?}",
                node.name,
                node.control_grpc_uri,
                started.elapsed().as_millis()
            );
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
        subscription_id: String::new(),
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
    node_error::global_error_from_detail(
        error,
        BaseErrorCode::Internal.code(),
        &format!("stream rpc {action} failed"),
        |msg| error!("{msg}"),
    )
}

fn rpc_status(error: tonic::Status, action: &str) -> GlobalError {
    node_error::global_error_from_tonic_status(
        error,
        &format!("stream rpc {action} failed"),
        |msg| error!("{msg}"),
    )
}

pub async fn init_media(
    node: &StreamNode,
    value: &MediaConfig,
    subscription_id: &str,
) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let mut request = request(value)?;
    request.subscription_id = subscription_id.to_string();
    base::log::debug!(
        "session rpc client outbound: method=stream_control.init_media, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .init_media(request)
        .await
        .map_err(|error| rpc_status(error, "init_media"))?
        .into_inner();
    ensure_unit(response, "init_media")
}

pub async fn create_output(
    node: &StreamNode,
    operation_id: &str,
    stream_id: &str,
    output_type: &str,
    audio_codec: &str,
    subscription_id: &str,
) -> GlobalResult<OutputInfo> {
    let mut client = client(node).await?;
    let request = CreateOutputRequest {
        operation: Some(OperationRef {
            operation_id: operation_id.to_string(),
            idempotency_key: operation_id.to_string(),
        }),
        stream_id: stream_id.to_string(),
        output_type: output_type.to_string(),
        endpoint_mode: EndpointMode::Single as i32,
        audio_codec: audio_codec.to_string(),
        subscription_id: subscription_id.to_string(),
    };
    base::log::debug!(
        "session rpc client outbound: method=stream_control.create_output, node={}, stream_id={}, output_type={}",
        node.name,
        stream_id,
        output_type
    );
    let response = client
        .create_output(request)
        .await
        .map_err(|error| rpc_status(error, "create_output"))?
        .into_inner();
    if let Some(error) = response.error {
        return Err(error_detail(error, "create_output"));
    }
    response.output.ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream rpc create_output returned no output",
            |msg| error!("{msg}: stream_id={stream_id}, output_type={output_type}"),
        )
    })
}

pub async fn release_subscription_outputs(
    node: &StreamNode,
    operation_id: &str,
    stream_id: &str,
    subscription_id: &str,
) -> GlobalResult<Vec<String>> {
    let mut client = client(node).await?;
    let response = client
        .release_subscription_outputs(ReleaseSubscriptionOutputsRequest {
            operation: Some(OperationRef {
                operation_id: operation_id.to_string(),
                idempotency_key: operation_id.to_string(),
            }),
            stream_id: stream_id.to_string(),
            subscription_id: subscription_id.to_string(),
        })
        .await
        .map_err(|error| rpc_status(error, "release_subscription_outputs"))?
        .into_inner();
    if let Some(error) = response.error {
        return Err(error_detail(error, "release_subscription_outputs"));
    }
    Ok(response.closed_output_ids)
}

pub async fn init_media_ext(node: &StreamNode, value: &MediaMap) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let request = request(value)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.init_media_ext, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .init_media_ext(request)
        .await
        .map_err(|error| rpc_status(error, "init_media_ext"))?
        .into_inner();
    ensure_unit(response, "init_media_ext")
}

pub async fn stream_online(node: &StreamNode, value: &StreamKey) -> GlobalResult<bool> {
    let mut client = client(node).await?;
    let request = request(value)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.stream_online, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .stream_online(request)
        .await
        .map_err(|error| rpc_status(error, "stream_online"))?
        .into_inner();
    ensure_bool(response, "stream_online")
}

pub async fn query_stream(node: &StreamNode, stream_id: &str) -> GlobalResult<QueryStreamResponse> {
    let mut client = client(node).await?;
    let response = client
        .query_stream(QueryStreamRequest {
            stream_id: stream_id.to_string(),
        })
        .await
        .map_err(|error| rpc_status(error, "query_stream"))?
        .into_inner();
    Ok(response)
}

pub async fn query_input_observation(
    node: &StreamNode,
    stream_id: &str,
    expected_ssrc: u32,
) -> GlobalResult<StreamInputObservation> {
    input_observation(
        query_stream(node, stream_id).await?,
        stream_id,
        expected_ssrc,
    )
}

pub async fn quiesce_receive_outputs(
    node: &StreamNode,
    stream_id: &str,
    expected_ssrc: u32,
    expected_generation: u64,
    reason: &str,
) -> GlobalResult<StreamInputObservation> {
    let response = staged_stop_receive(
        node,
        stream_id,
        reason,
        StopReceivePhase::QuiesceOutputs,
        expected_ssrc,
        expected_generation,
        0,
    )
    .await?;
    if let Some(error) = response.error.clone() {
        return Err(error_detail(error, "stop_receive"));
    }
    if !response.outputs_closed
        || response.input_removed
        || StreamState::try_from(response.state) != Ok(StreamState::Stopping)
    {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream outputs were not quiesced",
            |msg| {
                error!(
                    "{msg}: stream_id={stream_id}, state={}, outputs_closed={}, input_removed={}",
                    response.state, response.outputs_closed, response.input_removed
                )
            },
        ));
    }
    stop_observation(response, stream_id, expected_ssrc, expected_generation)
}

pub async fn finalize_receive(
    node: &StreamNode,
    stream_id: &str,
    expected_ssrc: u32,
    expected_generation: u64,
    expected_packet_count: u64,
    reason: &str,
) -> GlobalResult<FinalizeReceiveResult> {
    let response = staged_stop_receive(
        node,
        stream_id,
        reason,
        StopReceivePhase::Finalize,
        expected_ssrc,
        expected_generation,
        expected_packet_count,
    )
    .await?;
    if response
        .error
        .as_ref()
        .is_some_and(|error| error.code == "stream_input_changed")
    {
        return stop_observation(response, stream_id, expected_ssrc, expected_generation)
            .map(FinalizeReceiveResult::InputChanged);
    }
    if let Some(error) = response.error.clone() {
        return Err(error_detail(error, "stop_receive"));
    }
    if !response.outputs_closed
        || !response.input_removed
        || StreamState::try_from(response.state) != Ok(StreamState::Stopped)
    {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream finalize was not confirmed",
            |msg| {
                error!(
                    "{msg}: stream_id={stream_id}, state={}, outputs_closed={}, input_removed={}",
                    response.state, response.outputs_closed, response.input_removed
                )
            },
        ));
    }
    stop_observation(response, stream_id, expected_ssrc, expected_generation)
        .map(FinalizeReceiveResult::Finalized)
}

#[allow(clippy::too_many_arguments)]
async fn staged_stop_receive(
    node: &StreamNode,
    stream_id: &str,
    reason: &str,
    phase: StopReceivePhase,
    expected_ssrc: u32,
    expected_generation: u64,
    expected_packet_count: u64,
) -> GlobalResult<StopReceiveResponse> {
    let mut client = client(node).await?;
    let response = client
        .stop_receive(StopReceiveRequest {
            operation: Some(OperationRef {
                operation_id: format!("session-close-{stream_id}-{}", phase.as_str_name()),
                idempotency_key: format!("{stream_id}-{}", phase.as_str_name()),
            }),
            stream_id: stream_id.to_string(),
            reason: reason.to_string(),
            phase: phase as i32,
            expected_ssrc: expected_ssrc.to_string(),
            expected_lifecycle_generation: expected_generation,
            expected_packet_count,
        })
        .await
        .map_err(|error| rpc_status(error, "stop_receive"))?
        .into_inner();
    Ok(response)
}

fn input_observation(
    response: QueryStreamResponse,
    stream_id: &str,
    expected_ssrc: u32,
) -> GlobalResult<StreamInputObservation> {
    if !response.input_observed {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream input observation is unavailable",
            |msg| error!("{msg}: stream_id={stream_id}"),
        ));
    }
    observation_fields(
        &response.ssrc,
        response.lifecycle_generation,
        response.last_packet_at_ms,
        response.packet_count,
        response.input_idle_timeout_ms,
        stream_id,
        expected_ssrc,
    )
}

fn stop_observation(
    response: StopReceiveResponse,
    stream_id: &str,
    expected_ssrc: u32,
    expected_generation: u64,
) -> GlobalResult<StreamInputObservation> {
    let observation = observation_fields(
        &response.ssrc,
        response.lifecycle_generation,
        response.last_packet_at_ms,
        response.packet_count,
        response.input_idle_timeout_ms,
        stream_id,
        expected_ssrc,
    )?;
    if observation.lifecycle_generation != expected_generation {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream lifecycle generation changed",
            |msg| {
                error!(
                    "{msg}: stream_id={stream_id}, expected_generation={expected_generation}, actual_generation={}",
                    observation.lifecycle_generation
                )
            },
        ));
    }
    Ok(observation)
}

#[allow(clippy::too_many_arguments)]
fn observation_fields(
    ssrc: &str,
    lifecycle_generation: u64,
    last_packet_at_ms: u64,
    packet_count: u64,
    input_idle_timeout_ms: u64,
    stream_id: &str,
    expected_ssrc: u32,
) -> GlobalResult<StreamInputObservation> {
    let actual_ssrc = ssrc.parse::<u32>().map_err(|_| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream returned invalid SSRC",
            |msg| error!("{msg}: stream_id={stream_id}, ssrc={ssrc}"),
        )
    })?;
    if actual_ssrc != expected_ssrc || lifecycle_generation == 0 {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream input identity does not match dialog",
            |msg| {
                error!(
                    "{msg}: stream_id={stream_id}, expected_ssrc={expected_ssrc}, actual_ssrc={actual_ssrc}, lifecycle_generation={lifecycle_generation}"
                )
            },
        ));
    }
    Ok(StreamInputObservation {
        ssrc: actual_ssrc,
        lifecycle_generation,
        last_packet_at_ms,
        packet_count,
        input_idle_timeout_ms,
    })
}

pub async fn stop_receive(node: &StreamNode, stream_id: &str, reason: &str) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let response = client
        .stop_receive(StopReceiveRequest {
            operation: Some(OperationRef {
                operation_id: format!("session-reconcile-{stream_id}"),
                idempotency_key: stream_id.to_string(),
            }),
            stream_id: stream_id.to_string(),
            reason: reason.to_string(),
            phase: StopReceivePhase::Unspecified as i32,
            expected_ssrc: String::new(),
            expected_lifecycle_generation: 0,
            expected_packet_count: 0,
        })
        .await
        .map_err(|error| rpc_status(error, "stop_receive"))?
        .into_inner();
    if let Some(error) = response.error {
        return Err(error_detail(error, "stop_receive"));
    }
    if StreamState::try_from(response.state) != Ok(StreamState::Stopped) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream stop was not confirmed",
            |msg| error!("{msg}: stream_id={stream_id}, state={}", response.state),
        ));
    }
    let current = query_stream(node, stream_id).await?;
    if StreamState::try_from(current.state) == Ok(StreamState::Stopped) {
        Ok(())
    } else {
        Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream resource is still running after stop",
            |msg| error!("{msg}: stream_id={stream_id}, state={}", current.state),
        ))
    }
}

pub async fn record_info(
    node: &StreamNode,
    value: &StreamInfoQo,
) -> GlobalResult<StreamRecordInfo> {
    let mut client = client(node).await?;
    let request = request(value)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.record_info, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .record_info(request)
        .await
        .map_err(|error| rpc_status(error, "record_info"))?
        .into_inner();
    decode_json(response, "record_info")
}

pub async fn close_output(node: &StreamNode, value: &StreamInfoQo) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let request = request(value)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.close_output_by_ssrc, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .close_output_by_ssrc(request)
        .await
        .map_err(|error| rpc_status(error, "close_output"))?
        .into_inner();
    ensure_unit(response, "close_output")
}

pub async fn talk_open(node: &StreamNode, value: &TalkOpenReq) -> GlobalResult<TalkOpenResp> {
    let mut client = client(node).await?;
    let request = request(value)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.talk_open, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .talk_open(request)
        .await
        .map_err(|error| rpc_status(error, "talk_open"))?
        .into_inner();
    decode_json(response, "talk_open")
}

pub async fn talk_answer(node: &StreamNode, value: &TalkAnswerReq) -> GlobalResult<()> {
    let mut client = client(node).await?;
    let request = request(value)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.talk_answer, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .talk_answer(request)
        .await
        .map_err(|error| rpc_status(error, "talk_answer"))?
        .into_inner();
    ensure_unit(response, "talk_answer")
}

pub async fn talk_close(node: &StreamNode, talk_id: &str) -> GlobalResult<()> {
    let request = TalkCloseReq {
        talk_id: talk_id.to_string(),
    };
    let mut client = client(node).await?;
    let request = self::request(&request)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.talk_close, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .talk_close(request)
        .await
        .map_err(|error| rpc_status(error, "talk_close"))?
        .into_inner();
    ensure_unit(response, "talk_close")
}

pub async fn talk_online(node: &StreamNode, talk_id: &str) -> GlobalResult<bool> {
    let request = TalkCloseReq {
        talk_id: talk_id.to_string(),
    };
    let mut client = client(node).await?;
    let request = self::request(&request)?;
    base::log::debug!(
        "session rpc client outbound: method=stream_control.talk_online, node={}, req: payload_bytes={}",
        node.name,
        request.payload_json.len()
    );
    let response = client
        .talk_online(request)
        .await
        .map_err(|error| rpc_status(error, "talk_online"))?
        .into_inner();
    ensure_bool(response, "talk_online")
}
