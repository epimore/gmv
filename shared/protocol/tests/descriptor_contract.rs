use prost::Message;
use prost_types::FileDescriptorSet;
use std::collections::HashMap;

#[derive(Clone, PartialEq, Message)]
struct LegacyStartReceiveRequest {
    #[prost(message, optional, tag = "1")]
    operation: Option<gmv_protocol::common::v1::OperationRef>,
    #[prost(string, tag = "2")]
    stream_id: String,
    #[prost(string, tag = "3")]
    route_id: String,
    #[prost(string, tag = "4")]
    lease_id: String,
    #[prost(message, optional, tag = "5")]
    expected_stream: Option<gmv_protocol::common::v1::NodeIdentity>,
    #[prost(message, repeated, tag = "6")]
    preferred_endpoints: Vec<gmv_protocol::common::v1::Endpoint>,
}

fn descriptor() -> FileDescriptorSet {
    FileDescriptorSet::decode(gmv_protocol::FILE_DESCRIPTOR_SET).unwrap()
}

#[test]
fn descriptor_contains_versioned_packages() {
    let descriptor = descriptor();
    let packages = descriptor
        .file
        .iter()
        .map(|file| file.package.as_deref().unwrap_or_default())
        .collect::<Vec<_>>();

    for package in [
        "gmv.common.v1",
        "gmv.guard.v1",
        "gmv.session.v1",
        "gmv.stream.v1",
        "gmv.avai.v1",
    ] {
        assert!(packages.contains(&package), "missing package {package}");
    }
}

#[test]
fn node_identity_contains_instance_id_fencing_token() {
    let descriptor = descriptor();
    let common = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.common.v1"))
        .unwrap();
    let node_identity = common
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("NodeIdentity"))
        .unwrap();
    let instance_id = node_identity
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some("instance_id"))
        .unwrap();

    assert_eq!(instance_id.number, Some(2));
}

#[test]
fn enums_start_with_unspecified_zero_value() {
    let descriptor = descriptor();

    for file in descriptor.file {
        for item in file.enum_type {
            let enum_name = item.name.unwrap_or_default();
            let first = item
                .value
                .first()
                .unwrap_or_else(|| panic!("enum {enum_name} in {:?} has no values", file.name));
            assert_eq!(
                first.number,
                Some(0),
                "enum {enum_name} first value is not 0"
            );
            assert!(
                first
                    .name
                    .as_deref()
                    .unwrap_or_default()
                    .ends_with("UNSPECIFIED"),
                "enum {enum_name} first value must end with UNSPECIFIED"
            );
        }
    }
}

#[test]
fn guard_and_direct_service_rpc_boundaries_exist() {
    let descriptor = descriptor();
    let services = descriptor
        .file
        .iter()
        .flat_map(|file| {
            let package = file.package.clone().unwrap_or_default();
            file.service.iter().map(move |service| {
                format!("{package}.{}", service.name.as_deref().unwrap_or_default())
            })
        })
        .collect::<Vec<_>>();

    for service in [
        "gmv.guard.v1.GuardNodeControl",
        "gmv.guard.v1.GuardControl",
        "gmv.session.v1.SessionControl",
        "gmv.stream.v1.StreamControl",
        "gmv.avai.v1.AvaiControl",
    ] {
        assert!(
            services.contains(&service.to_string()),
            "missing service {service}"
        );
    }
}

#[test]
fn session_resource_override_rpcs_are_stable() {
    let descriptor = descriptor();
    let session = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.session.v1"))
        .unwrap();
    let service = session
        .service
        .iter()
        .find(|service| service.name.as_deref() == Some("SessionControl"))
        .unwrap();
    let methods = service
        .method
        .iter()
        .filter_map(|method| method.name.as_deref())
        .collect::<Vec<_>>();
    for method in [
        "ListGbResources",
        "SaveGbResourceConfirmation",
        "ResetGbResourceConfirmation",
        "RefreshPlaybackPresence",
    ] {
        assert!(methods.contains(&method), "missing SessionControl.{method}");
    }
    let set_state = session
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("SetPlaybackStateRequest"))
        .unwrap();
    assert_eq!(
        set_state
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some("subscription_id"))
            .unwrap()
            .number,
        Some(6)
    );
}

#[test]
fn session_record_query_contract_is_stable() {
    let descriptor = descriptor();
    let session = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.session.v1"))
        .unwrap();
    let service = session
        .service
        .iter()
        .find(|service| service.name.as_deref() == Some("SessionControl"))
        .unwrap();
    let methods = service
        .method
        .iter()
        .filter_map(|method| method.name.as_deref())
        .collect::<Vec<_>>();
    for method in ["GetGbChannelRecords", "QueryGbChannelRecords"] {
        assert!(methods.contains(&method), "missing SessionControl.{method}");
    }
    let request = session
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("GetGbChannelRecordsRequest"))
        .unwrap();
    for (field, number) in [
        ("device_id", 1),
        ("channel_id", 2),
        ("start_time_sec", 3),
        ("end_time_sec", 4),
        ("page", 5),
        ("page_size", 6),
    ] {
        assert_eq!(
            request
                .field
                .iter()
                .find(|item| item.name.as_deref() == Some(field))
                .unwrap()
                .number,
            Some(number)
        );
    }
    let response = session
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("GetGbChannelRecordsResponse"))
        .unwrap();
    for (field, number) in [
        ("current_batch", 1),
        ("attempt_batch", 2),
        ("segments", 3),
        ("next_query_at_ms", 4),
        ("server_time_ms", 5),
        ("total", 6),
        ("page", 7),
        ("page_size", 8),
    ] {
        assert_eq!(
            response
                .field
                .iter()
                .find(|item| item.name.as_deref() == Some(field))
                .unwrap()
                .number,
            Some(number)
        );
    }
}

#[test]
fn session_image_access_contract_is_stable() {
    let descriptor = descriptor();
    let session = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.session.v1"))
        .unwrap();
    let service = session
        .service
        .iter()
        .find(|service| service.name.as_deref() == Some("SessionControl"))
        .unwrap();
    assert!(
        service
            .method
            .iter()
            .any(|method| method.name.as_deref() == Some("IssueGbChannelImageAccess"))
    );
    assert!(
        service
            .method
            .iter()
            .any(|method| method.name.as_deref() == Some("SetGbChannelCover"))
    );

    let image = session
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("GbChannelImage"))
        .unwrap();
    for (field, number) in [
        ("file_name", 6),
        ("content_type", 7),
        ("file_size", 8),
        ("can_preview", 9),
        ("session_node_id", 10),
    ] {
        assert_eq!(
            image
                .field
                .iter()
                .find(|item| item.name.as_deref() == Some(field))
                .unwrap()
                .number,
            Some(number)
        );
    }

    let channel = session
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("GbChannel"))
        .unwrap();
    assert_eq!(
        channel
            .field
            .iter()
            .find(|item| item.name.as_deref() == Some("cover_image_id"))
            .unwrap()
            .number,
        Some(30)
    );

    let list_request = session
        .message_type
        .iter()
        .find(|message| message.name.as_deref() == Some("ListGbChannelImagesRequest"))
        .unwrap();
    for (field, number) in [
        ("start_time_ms", 3),
        ("end_time_ms", 4),
        ("page", 5),
        ("page_size", 6),
    ] {
        assert_eq!(
            list_request
                .field
                .iter()
                .find(|item| item.name.as_deref() == Some(field))
                .unwrap()
                .number,
            Some(number)
        );
    }
}

#[test]
fn session_stream_monitoring_contract_is_stable() {
    let descriptor = descriptor();
    let session = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.session.v1"))
        .unwrap();
    let service = session
        .service
        .iter()
        .find(|service| service.name.as_deref() == Some("SessionControl"))
        .unwrap();
    let methods = service
        .method
        .iter()
        .filter_map(|method| method.name.as_deref())
        .collect::<Vec<_>>();
    for method in [
        "ListActiveStreams",
        "ListActiveStreamDialogs",
        "GetActiveStreamManagement",
        "ListStreamHistory",
    ] {
        assert!(methods.contains(&method), "missing SessionControl.{method}");
    }

    let field_number = |message_name: &str, field_name: &str| {
        session
            .message_type
            .iter()
            .find(|message| message.name.as_deref() == Some(message_name))
            .unwrap()
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some(field_name))
            .unwrap()
            .number
    };
    assert_eq!(
        field_number("StopDeviceStreamRequest", "expected_session"),
        Some(6)
    );
    assert_eq!(
        field_number("StopDeviceStreamRequest", "stop_reason"),
        Some(7)
    );
    assert_eq!(
        field_number("ListActiveStreamsRequest", "expected_session"),
        Some(9)
    );
    assert_eq!(
        field_number("ListStreamHistoryRequest", "expected_session"),
        Some(9)
    );
    assert_eq!(
        field_number("ListActiveStreamDialogsRequest", "expected_session"),
        Some(9)
    );
    assert_eq!(
        field_number("GetActiveStreamManagementRequest", "expected_session"),
        Some(2)
    );
    assert_eq!(
        field_number("StreamHistoryItem", "terminal_reason"),
        Some(13)
    );
    assert_eq!(field_number("StreamHistoryItem", "error_code"), Some(14));
    assert_eq!(
        field_number("StreamHistoryItem", "terminal_reason_label"),
        Some(16)
    );
    assert_eq!(field_number("StreamHistoryItem", "stop_reason"), Some(17));
    assert_eq!(field_number("ActiveStreamItem", "viewer_count"), Some(17));
    assert_eq!(field_number("ActiveStreamItem", "viewer_formats"), Some(18));
    assert_eq!(
        field_number("ActiveStreamItem", "supported_formats"),
        Some(19)
    );
    assert_eq!(field_number("ActiveStreamItem", "output_format"), Some(20));
}

#[test]
fn node_heartbeat_contains_structured_host_metrics() {
    let descriptor = descriptor();
    let guard = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.guard.v1"))
        .unwrap();
    let message = |name: &str| {
        guard
            .message_type
            .iter()
            .find(|message| message.name.as_deref() == Some(name))
            .unwrap()
    };
    let field_number = |message_name: &str, field_name: &str| {
        message(message_name)
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some(field_name))
            .unwrap()
            .number
    };
    assert_eq!(field_number("NodeHeartbeat", "host_metrics"), Some(3));
    assert_eq!(field_number("HostMetrics", "cpu_usage_percent"), Some(1));
    assert_eq!(field_number("HostMetrics", "process_threads"), Some(14));
}

#[test]
fn stream_output_lifecycle_contract_is_stable() {
    let descriptor = descriptor();
    let stream = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.stream.v1"))
        .unwrap();
    let message = |name: &str| {
        stream
            .message_type
            .iter()
            .find(|message| message.name.as_deref() == Some(name))
            .unwrap()
    };
    let field_number = |message_name: &str, field_name: &str| {
        message(message_name)
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some(field_name))
            .unwrap()
            .number
    };
    assert_eq!(field_number("CreateOutputRequest", "audio_codec"), Some(5));
    assert_eq!(
        field_number("CreateOutputRequest", "subscription_id"),
        Some(6)
    );
    assert_eq!(field_number("CreateOutputResponse", "output"), Some(4));
    assert_eq!(field_number("CloseOutputRequest", "stream_id"), Some(3));
    assert_eq!(field_number("StopReceiveRequest", "phase"), Some(4));
    assert_eq!(field_number("StopReceiveRequest", "expected_ssrc"), Some(5));
    assert_eq!(
        field_number("StopReceiveRequest", "expected_lifecycle_generation"),
        Some(6)
    );
    assert_eq!(
        field_number("StopReceiveRequest", "expected_packet_count"),
        Some(7)
    );
    assert_eq!(
        field_number("StopReceiveRequest", "expected_lease_id"),
        Some(8)
    );
    assert_eq!(
        field_number("StopReceiveRequest", "expected_route_id"),
        Some(9)
    );
    assert_eq!(
        field_number("StopReceiveResponse", "outputs_closed"),
        Some(3)
    );
    assert_eq!(
        field_number("StopReceiveResponse", "input_removed"),
        Some(4)
    );
    assert_eq!(
        field_number("StopReceiveResponse", "input_idle_timeout_ms"),
        Some(9)
    );
    assert_eq!(field_number("QueryStreamResponse", "viewer_count"), Some(9));
    assert_eq!(
        field_number("QueryStreamResponse", "viewer_formats"),
        Some(10)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "primary_output_format"),
        Some(17)
    );
    assert_eq!(field_number("QueryStreamResponse", "ssrc"), Some(11));
    assert_eq!(
        field_number("QueryStreamResponse", "lifecycle_generation"),
        Some(12)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "last_packet_at_ms"),
        Some(13)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "packet_count"),
        Some(14)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "input_idle_timeout_ms"),
        Some(15)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "input_observed"),
        Some(16)
    );
    assert_eq!(
        field_number("GetPlaybackEndpointsResponse", "outputs"),
        Some(2)
    );
    assert_eq!(field_number("OutputInfo", "output_id"), Some(1));
    assert_eq!(field_number("OutputInfo", "state"), Some(5));
    assert_eq!(field_number("OutputInfo", "subscription_id"), Some(6));
    assert_eq!(
        field_number("StreamJsonRequest", "subscription_id"),
        Some(2)
    );
}

#[test]
fn stream_receive_allocation_contract_is_additive() {
    let descriptor = descriptor();
    let stream = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.stream.v1"))
        .unwrap();
    let message = |name: &str| {
        stream
            .message_type
            .iter()
            .find(|message| message.name.as_deref() == Some(name))
            .unwrap()
    };
    let field = |message_name: &str, field_name: &str| {
        message(message_name)
            .field
            .iter()
            .find(|field| field.name.as_deref() == Some(field_name))
            .unwrap()
    };

    for (name, number) in [
        ("operation", 1),
        ("stream_id", 2),
        ("route_id", 3),
        ("lease_id", 4),
        ("expected_stream", 5),
        ("preferred_endpoints", 6),
        ("constraints", 7),
        ("reservation_ttl_ms", 8),
    ] {
        assert_eq!(field("StartReceiveRequest", name).number, Some(number));
    }
    assert_eq!(
        field("StartReceiveResponse", "receive_endpoints").number,
        Some(3)
    );
    assert_eq!(
        field("StartReceiveResponse", "receive_endpoints")
            .type_name
            .as_deref(),
        Some(".gmv.common.v1.Endpoint")
    );
}

#[test]
fn node_capability_and_allocated_endpoint_use_distinct_contract_paths() {
    let descriptor = descriptor();
    let guard_messages = descriptor
        .file
        .iter()
        .filter(|file| file.package.as_deref() == Some("gmv.guard.v1"))
        .flat_map(|file| file.message_type.iter())
        .collect::<Vec<_>>();
    let register_endpoints = guard_messages
        .iter()
        .find(|message| message.name.as_deref() == Some("RegisterNodeRequest"))
        .unwrap()
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some("endpoints"))
        .unwrap();
    let allocate_endpoints = guard_messages
        .iter()
        .find(|message| message.name.as_deref() == Some("AllocateStreamResponse"))
        .unwrap()
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some("endpoints"))
        .unwrap();

    assert_eq!(register_endpoints.number, Some(4));
    assert_eq!(allocate_endpoints.number, Some(4));
    assert_eq!(
        register_endpoints.type_name.as_deref(),
        Some(".gmv.common.v1.Endpoint")
    );
    assert_eq!(
        allocate_endpoints.type_name.as_deref(),
        Some(".gmv.common.v1.Endpoint")
    );
}

#[test]
fn start_receive_request_is_wire_compatible_with_legacy_callers() {
    let legacy = LegacyStartReceiveRequest {
        operation: Some(gmv_protocol::common::v1::OperationRef {
            operation_id: "operation-1".to_string(),
            idempotency_key: "idempotency-1".to_string(),
        }),
        stream_id: "stream-1".to_string(),
        route_id: "route-1".to_string(),
        lease_id: "lease-1".to_string(),
        expected_stream: Some(gmv_protocol::common::v1::NodeIdentity {
            node_id: "stream-node-1".to_string(),
            instance_id: "instance-1".to_string(),
            kind: gmv_protocol::common::v1::NodeKind::Stream.into(),
        }),
        preferred_endpoints: Vec::new(),
    };

    let decoded =
        gmv_protocol::stream::v1::StartReceiveRequest::decode(legacy.encode_to_vec().as_slice())
            .unwrap();
    assert_eq!(decoded.stream_id, legacy.stream_id);
    assert_eq!(decoded.route_id, legacy.route_id);
    assert_eq!(decoded.lease_id, legacy.lease_id);
    assert!(decoded.constraints.is_empty());
    assert_eq!(decoded.reservation_ttl_ms, 0);

    let mut constraints = HashMap::new();
    constraints.insert("transport".to_string(), "tcp_passive".to_string());
    let current = gmv_protocol::stream::v1::StartReceiveRequest {
        operation: legacy.operation.clone(),
        stream_id: legacy.stream_id.clone(),
        route_id: legacy.route_id.clone(),
        lease_id: legacy.lease_id.clone(),
        expected_stream: legacy.expected_stream.clone(),
        preferred_endpoints: legacy.preferred_endpoints.clone(),
        constraints,
        reservation_ttl_ms: 30_000,
    };
    let decoded_legacy =
        LegacyStartReceiveRequest::decode(current.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded_legacy.stream_id, current.stream_id);
    assert_eq!(decoded_legacy.route_id, current.route_id);
    assert_eq!(decoded_legacy.lease_id, current.lease_id);
}
