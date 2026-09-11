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

#[derive(Clone, PartialEq, Message)]
struct LegacyTypedCommand {
    #[prost(string, tag = "1")]
    command_id: String,
    #[prost(int32, tag = "2")]
    command_type: i32,
    #[prost(int64, tag = "3")]
    deadline_epoch_ms: i64,
    #[prost(string, tag = "4")]
    expected_manifest_revision: String,
}

#[derive(Clone, PartialEq, Message)]
struct LegacyCommandReceipt {
    #[prost(string, tag = "1")]
    command_id: String,
    #[prost(int32, tag = "2")]
    state: i32,
    #[prost(string, tag = "3")]
    stable_error_code: String,
    #[prost(bytes = "vec", tag = "4")]
    result: Vec<u8>,
    #[prost(int64, tag = "5")]
    completed_at_epoch_ms: i64,
}

fn descriptor() -> FileDescriptorSet {
    FileDescriptorSet::decode(gmv_protocol::FILE_DESCRIPTOR_SET).unwrap()
}

fn descriptor_file<'a>(
    descriptor: &'a FileDescriptorSet,
    package: &str,
) -> &'a prost_types::FileDescriptorProto {
    descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some(package))
        .unwrap()
}

fn descriptor_message<'a>(
    file: &'a prost_types::FileDescriptorProto,
    name: &str,
) -> &'a prost_types::DescriptorProto {
    file.message_type
        .iter()
        .find(|message| message.name.as_deref() == Some(name))
        .unwrap()
}

fn descriptor_field_number(message: &prost_types::DescriptorProto, name: &str) -> Option<i32> {
    message
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some(name))
        .and_then(|field| field.number)
}

fn descriptor_enum_value_number(
    file: &prost_types::FileDescriptorProto,
    enum_name: &str,
    value_name: &str,
) -> Option<i32> {
    file.enum_type
        .iter()
        .find(|item| item.name.as_deref() == Some(enum_name))?
        .value
        .iter()
        .find(|value| value.name.as_deref() == Some(value_name))?
        .number
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
        "gmv.center_agent.v1",
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
fn gmv_center_agent_contract_has_stable_identity_and_typed_commands() {
    let descriptor = descriptor();
    let guard = descriptor_file(&descriptor, "gmv.guard.v1");
    let register = descriptor_message(guard, "RegisterNodeRequest");
    assert_eq!(
        descriptor_field_number(register, "installation_id"),
        Some(12)
    );
    assert_eq!(descriptor_field_number(register, "host_id"), Some(13));

    let gmv_center_agent = descriptor_file(&descriptor, "gmv.center_agent.v1");
    let center_message = descriptor_message(gmv_center_agent, "CenterToGmvCenterAgentMessage");
    assert_eq!(
        descriptor_field_number(center_message, "expected_gmv_center_agent_instance_id"),
        Some(5)
    );
    assert_eq!(descriptor_field_number(center_message, "host_id"), Some(7));
    assert_eq!(
        descriptor_field_number(center_message, "receipt_ack"),
        Some(12)
    );
    assert_eq!(
        descriptor_field_number(center_message, "upgrade_decision"),
        Some(13)
    );
    let gmv_center_agent_message =
        descriptor_message(gmv_center_agent, "GmvCenterAgentToCenterMessage");
    assert_eq!(
        descriptor_field_number(gmv_center_agent_message, "upgrade_request"),
        Some(15)
    );
    assert_eq!(
        descriptor_field_number(gmv_center_agent_message, "presence"),
        Some(16)
    );
    assert_eq!(
        descriptor_field_number(gmv_center_agent_message, "host_id"),
        Some(7)
    );
    let receipt_ack = descriptor_message(gmv_center_agent, "ReceiptAck");
    assert_eq!(
        descriptor_field_number(receipt_ack, "receipt_message_id"),
        Some(1)
    );
    let command = descriptor_message(gmv_center_agent, "TypedCommand");
    for (field, number) in [
        ("command_id", 1),
        ("command_type", 2),
        ("deadline_epoch_ms", 3),
        ("expected_manifest_revision", 4),
        ("operation_id", 5),
        ("installation_id", 6),
        ("host_id", 7),
        ("component_id", 8),
        ("inventory_refresh", 10),
        ("health_snapshot", 11),
        ("service_start", 12),
        ("service_stop", 13),
        ("service_restart", 14),
        ("log_query", 15),
        ("diagnostic_collect", 16),
    ] {
        assert_eq!(descriptor_field_number(command, field), Some(number));
    }
    assert!(descriptor_field_number(command, "shell").is_none());
    assert!(descriptor_field_number(command, "command_line").is_none());
    assert!(descriptor_field_number(command, "argv").is_none());

    let receipt = descriptor_message(gmv_center_agent, "CommandReceipt");
    for (field, number) in [
        ("command_id", 1),
        ("state", 2),
        ("stable_error_code", 3),
        ("result", 4),
        ("completed_at_epoch_ms", 5),
        ("operation_id", 6),
        ("component_id", 7),
        ("started_at_epoch_ms", 8),
        ("steps", 9),
        ("result_summary", 10),
        ("result_artifact", 11),
        ("failure_reason", 12),
    ] {
        assert_eq!(descriptor_field_number(receipt, field), Some(number));
    }

    for (value, number) in [
        ("GMV_CENTER_AGENT_COMMAND_TYPE_UNSPECIFIED", 0),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_INVENTORY_REFRESH", 1),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_HEALTH_SNAPSHOT", 2),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_SERVICE_START", 3),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_SERVICE_STOP", 4),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_SERVICE_RESTART", 5),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_LOG_QUERY", 6),
        ("GMV_CENTER_AGENT_COMMAND_TYPE_DIAGNOSTIC_COLLECT", 7),
    ] {
        assert_eq!(
            descriptor_enum_value_number(gmv_center_agent, "GmvCenterAgentCommandType", value),
            Some(number)
        );
    }
    for (value, number) in [
        ("COMMAND_FAILURE_REASON_UNSPECIFIED", 0),
        ("COMMAND_FAILURE_REASON_DEADLINE_EXPIRED", 1),
        ("COMMAND_FAILURE_REASON_UNKNOWN_COMPONENT", 2),
        ("COMMAND_FAILURE_REASON_UNSUPPORTED_CAPABILITY", 3),
        ("COMMAND_FAILURE_REASON_MANIFEST_MISMATCH", 4),
        ("COMMAND_FAILURE_REASON_ALREADY_TERMINAL", 5),
        ("COMMAND_FAILURE_REASON_BUSY_CONFLICT", 6),
    ] {
        assert_eq!(
            descriptor_enum_value_number(gmv_center_agent, "CommandFailureReason", value),
            Some(number)
        );
    }

    for message_name in [
        "InventoryRefreshRequest",
        "HealthSnapshotRequest",
        "ServiceStartRequest",
        "ServiceStopRequest",
        "ServiceRestartRequest",
        "LogQueryRequest",
        "DiagnosticCollectRequest",
    ] {
        let request = descriptor_message(gmv_center_agent, message_name);
        for field in &request.field {
            assert!(
                !["shell", "command_line", "argv", "unit", "path", "url"]
                    .contains(&field.name.as_deref().unwrap_or_default()),
                "{message_name} exposes an unsafe execution field"
            );
        }
    }
}

#[test]
fn gmv_center_agent_typed_command_payloads_roundtrip() {
    use gmv_protocol::gmv_center_agent::v1::{
        DiagnosticCollectRequest, GmvCenterAgentCommandType, HealthSnapshotRequest,
        InventoryRefreshRequest, LogQueryRequest, ServiceRestartRequest, ServiceStartRequest,
        ServiceStopRequest, TypedCommand, typed_command,
    };

    let requests = [
        (
            GmvCenterAgentCommandType::InventoryRefresh,
            typed_command::Request::InventoryRefresh(InventoryRefreshRequest {}),
        ),
        (
            GmvCenterAgentCommandType::HealthSnapshot,
            typed_command::Request::HealthSnapshot(HealthSnapshotRequest {}),
        ),
        (
            GmvCenterAgentCommandType::ServiceStart,
            typed_command::Request::ServiceStart(ServiceStartRequest {}),
        ),
        (
            GmvCenterAgentCommandType::ServiceStop,
            typed_command::Request::ServiceStop(ServiceStopRequest {}),
        ),
        (
            GmvCenterAgentCommandType::ServiceRestart,
            typed_command::Request::ServiceRestart(ServiceRestartRequest {}),
        ),
        (
            GmvCenterAgentCommandType::LogQuery,
            typed_command::Request::LogQuery(LogQueryRequest {
                since_epoch_ms: 1_000,
                until_epoch_ms: 2_000,
                tail_lines: 100,
                level: "WARN".to_string(),
                keyword: "action=restart".to_string(),
                max_lines: 500,
                max_bytes: 1_048_576,
            }),
        ),
        (
            GmvCenterAgentCommandType::DiagnosticCollect,
            typed_command::Request::DiagnosticCollect(DiagnosticCollectRequest {}),
        ),
    ];

    for (command_type, request) in requests {
        let command = TypedCommand {
            command_id: "command-1".to_string(),
            command_type: command_type as i32,
            deadline_epoch_ms: 2_000,
            expected_manifest_revision: "manifest-7".to_string(),
            operation_id: "operation-1".to_string(),
            installation_id: "installation-1".to_string(),
            host_id: "host-1".to_string(),
            component_id: "stream".to_string(),
            request: Some(request),
        };
        let encoded = command.encode_to_vec();
        assert_eq!(TypedCommand::decode(encoded.as_slice()).unwrap(), command);
    }
}

#[test]
#[allow(deprecated)]
fn gmv_center_agent_command_extension_is_wire_compatible() {
    use gmv_protocol::gmv_center_agent::v1::{
        CommandFailureReason, CommandReceipt, CommandResultArtifactRef, CommandState,
        CommandStepResult, CommandStepState, GmvCenterAgentCommandType, ServiceStopRequest,
        TypedCommand, typed_command,
    };

    let legacy_command = LegacyTypedCommand {
        command_id: "legacy-command".to_string(),
        command_type: GmvCenterAgentCommandType::HealthSnapshot as i32,
        deadline_epoch_ms: 1_000,
        expected_manifest_revision: "manifest-1".to_string(),
    };
    let decoded = TypedCommand::decode(legacy_command.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.command_id, legacy_command.command_id);
    assert_eq!(decoded.command_type, legacy_command.command_type);
    assert_eq!(decoded.deadline_epoch_ms, legacy_command.deadline_epoch_ms);
    assert_eq!(
        decoded.expected_manifest_revision,
        legacy_command.expected_manifest_revision
    );
    assert!(decoded.operation_id.is_empty());
    assert!(decoded.component_id.is_empty());
    assert!(decoded.request.is_none());

    let legacy_receipt = LegacyCommandReceipt {
        command_id: "legacy-command".to_string(),
        state: CommandState::Succeeded as i32,
        stable_error_code: String::new(),
        result: b"legacy-result".to_vec(),
        completed_at_epoch_ms: 1_100,
    };
    let decoded = CommandReceipt::decode(legacy_receipt.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.command_id, legacy_receipt.command_id);
    assert_eq!(decoded.state, legacy_receipt.state);
    assert_eq!(decoded.result, legacy_receipt.result);
    assert!(decoded.operation_id.is_empty());
    assert!(decoded.steps.is_empty());
    assert!(decoded.result_artifact.is_none());

    let current_command = TypedCommand {
        command_id: "current-command".to_string(),
        command_type: GmvCenterAgentCommandType::ServiceStop as i32,
        deadline_epoch_ms: 2_000,
        expected_manifest_revision: "manifest-2".to_string(),
        operation_id: "operation-2".to_string(),
        installation_id: "installation-1".to_string(),
        host_id: "host-1".to_string(),
        component_id: "guard".to_string(),
        request: Some(typed_command::Request::ServiceStop(ServiceStopRequest {})),
    };
    let legacy_decoded =
        LegacyTypedCommand::decode(current_command.encode_to_vec().as_slice()).unwrap();
    assert_eq!(legacy_decoded.command_id, current_command.command_id);
    assert_eq!(legacy_decoded.command_type, current_command.command_type);
    assert_eq!(
        legacy_decoded.expected_manifest_revision,
        current_command.expected_manifest_revision
    );

    let current_receipt = CommandReceipt {
        command_id: "current-command".to_string(),
        state: CommandState::Rejected as i32,
        stable_error_code: "manifest_mismatch".to_string(),
        result: Vec::new(),
        completed_at_epoch_ms: 2_100,
        operation_id: "operation-2".to_string(),
        component_id: "guard".to_string(),
        started_at_epoch_ms: 2_000,
        steps: vec![CommandStepResult {
            step_name: "validate_manifest".to_string(),
            state: CommandStepState::Failed as i32,
            started_at_epoch_ms: 2_000,
            completed_at_epoch_ms: 2_100,
            failure_reason: CommandFailureReason::ManifestMismatch as i32,
            stable_error_code: "manifest_mismatch".to_string(),
            result_summary: "manifest revision changed".to_string(),
        }],
        result_summary: "command rejected before execution".to_string(),
        result_artifact: Some(CommandResultArtifactRef {
            artifact_id: "diagnostic-operation-2".to_string(),
            media_type: "application/zstd".to_string(),
            content_size: 4_096,
            sha256: "0123456789abcdef".to_string(),
        }),
        failure_reason: CommandFailureReason::ManifestMismatch as i32,
    };
    let legacy_receipt_view =
        LegacyCommandReceipt::decode(current_receipt.encode_to_vec().as_slice()).unwrap();
    assert_eq!(legacy_receipt_view.command_id, current_receipt.command_id);
    assert_eq!(legacy_receipt_view.state, current_receipt.state);
    assert_eq!(
        legacy_receipt_view.completed_at_epoch_ms,
        current_receipt.completed_at_epoch_ms
    );
}

#[test]
fn gmv_center_agent_unknown_command_enum_value_is_preserved() {
    use gmv_protocol::gmv_center_agent::v1::TypedCommand;

    let command = TypedCommand {
        command_id: "future-command".to_string(),
        command_type: 99_999,
        deadline_epoch_ms: 1_000,
        expected_manifest_revision: String::new(),
        operation_id: String::new(),
        installation_id: String::new(),
        host_id: String::new(),
        component_id: String::new(),
        request: None,
    };
    let decoded = TypedCommand::decode(command.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded.command_type, 99_999);
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
    assert!(
        !services.contains(&"gmv.center_agent.v1.GmvCenterAgentGateway".to_string()),
        "GmvCenterAgent/GMVC transport is MQTT, not a protobuf RPC service"
    );
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
fn session_device_online_status_filter_is_optional_and_appended() {
    let descriptor = descriptor();
    let session = descriptor_file(&descriptor, "gmv.session.v1");
    let snapshot = descriptor_message(session, "SnapshotImageResponse");
    assert_eq!(descriptor_field_number(snapshot, "image_ids"), Some(3));
    let request = descriptor_message(session, "ListGbDevicesRequest");
    let field = request
        .field
        .iter()
        .find(|field| field.name.as_deref() == Some("monitor_status"))
        .unwrap();

    assert_eq!(field.number, Some(7));
    assert_eq!(field.proto3_optional, Some(true));
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
fn live_stream_profile_contract_is_stable() {
    let descriptor = descriptor();
    let session = descriptor_file(&descriptor, "gmv.session.v1");
    let request = descriptor_message(session, "StartDeviceStreamRequest");
    assert_eq!(
        descriptor_field_number(request, "video_stream_profile"),
        Some(21)
    );

    let response = descriptor_message(session, "DeviceStreamResponse");
    assert_eq!(descriptor_field_number(response, "video_codec"), Some(5));
    assert_eq!(descriptor_field_number(response, "audio_codec"), Some(6));
    assert_eq!(
        descriptor_field_number(response, "requested_stream_profile"),
        Some(13)
    );
    assert_eq!(
        descriptor_field_number(response, "effective_stream_profile"),
        Some(14)
    );
    assert_eq!(
        descriptor_field_number(response, "stream_profile_verification"),
        Some(15)
    );

    let active = descriptor_message(session, "ActiveStreamItem");
    assert_eq!(
        descriptor_field_number(active, "requested_stream_profile"),
        Some(21)
    );
    assert_eq!(
        descriptor_field_number(active, "effective_stream_profile"),
        Some(22)
    );
    assert_eq!(
        descriptor_field_number(active, "stream_profile_verification"),
        Some(23)
    );

    let stream = descriptor_file(&descriptor, "gmv.stream.v1");
    let query = descriptor_message(stream, "QueryStreamResponse");
    assert_eq!(descriptor_field_number(query, "readiness_stage"), Some(18));
    assert_eq!(descriptor_field_number(query, "queue_drop_count"), Some(25));
    assert_eq!(descriptor_field_number(query, "audio_codec"), Some(26));
    assert_eq!(descriptor_field_number(query, "mime_codec"), Some(27));
    assert_eq!(descriptor_field_number(response, "mime_codec"), Some(16));
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
    assert_eq!(field_number("OutputInfo", "failure"), Some(10));
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
    assert_eq!(field_number("OutputInfo", "video_codec"), Some(7));
    assert_eq!(field_number("OutputInfo", "audio_codec"), Some(8));
    assert_eq!(field_number("OutputInfo", "mime_codec"), Some(9));
    assert_eq!(field_number("OutputInfo", "source_audio_state"), Some(11));
    assert_eq!(field_number("OutputInfo", "output_audio_mode"), Some(12));
    assert_eq!(field_number("OutputInfo", "generation"), Some(17));
    assert_eq!(
        field_number("QueryStreamResponse", "source_audio_state"),
        Some(28)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "output_audio_mode"),
        Some(29)
    );
    assert_eq!(
        field_number("QueryStreamResponse", "output_generation"),
        Some(34)
    );
    assert_eq!(
        field_number("StreamJsonRequest", "subscription_id"),
        Some(2)
    );
}

#[test]
fn avai_multisource_task_contract_is_additive_and_typed() {
    let descriptor = descriptor();
    let common = descriptor_file(&descriptor, "gmv.common.v1");
    let endpoint = descriptor_message(common, "DataEndpoint");
    assert_eq!(descriptor_field_number(endpoint, "uri"), Some(2));
    assert_eq!(descriptor_field_number(endpoint, "capabilities"), Some(3));
    let grant = descriptor_message(common, "AccessGrant");
    assert_eq!(descriptor_field_number(grant, "expected_consumer"), Some(2));
    assert_eq!(descriptor_field_number(grant, "endpoints"), Some(5));
    assert_eq!(descriptor_field_number(grant, "proof"), Some(6));

    let avai = descriptor_file(&descriptor, "gmv.avai.v1");
    let create = descriptor_message(avai, "CreateTaskRequest");
    for (field, number) in [
        ("task_type", 3),
        ("payload", 6),
        ("capability", 7),
        ("requested_model", 8),
        ("source", 9),
        ("domain_config", 10),
        ("deadline_epoch_ms", 11),
    ] {
        assert_eq!(descriptor_field_number(create, field), Some(number));
    }
    let source = descriptor_message(avai, "SourceSpec");
    assert_eq!(source.oneof_decl.len(), 1);
    for (field, number) in [("owned_image", 1), ("image_url", 2), ("stream_frame", 3)] {
        let field = source
            .field
            .iter()
            .find(|item| item.name.as_deref() == Some(field))
            .unwrap();
        assert_eq!(field.number, Some(number));
        assert_eq!(field.oneof_index, Some(0));
    }
    let query = descriptor_message(avai, "QueryTaskResponse");
    assert_eq!(descriptor_field_number(query, "result"), Some(3));
    assert_eq!(descriptor_field_number(query, "typed_result"), Some(5));
    let prepare_upload = descriptor_message(avai, "PrepareImageUploadRequest");
    assert_eq!(
        descriptor_field_number(prepare_upload, "expected_avai"),
        Some(2)
    );
    assert_eq!(
        descriptor_field_number(prepare_upload, "deadline_epoch_ms"),
        Some(6)
    );
    let upload_ticket = descriptor_message(avai, "ImageUploadTicket");
    assert_eq!(descriptor_field_number(upload_ticket, "endpoint"), Some(2));
    assert_eq!(descriptor_field_number(upload_ticket, "proof"), Some(3));
    let finalize_upload = descriptor_message(avai, "FinalizeImageUploadResponse");
    assert_eq!(descriptor_field_number(finalize_upload, "source"), Some(1));

    let session = descriptor_file(&descriptor, "gmv.session.v1");
    let service = session
        .service
        .iter()
        .find(|service| service.name.as_deref() == Some("SessionControl"))
        .unwrap();
    assert!(
        service
            .method
            .iter()
            .any(|method| method.name.as_deref() == Some("IssueGbChannelImageSourceAccess"))
    );
    let request = descriptor_message(session, "IssueGbChannelImageSourceAccessRequest");
    assert_eq!(
        descriptor_field_number(request, "expected_consumer"),
        Some(5)
    );
    assert_eq!(
        descriptor_field_number(request, "deadline_epoch_ms"),
        Some(7)
    );
    let response = descriptor_message(session, "IssueGbChannelImageSourceAccessResponse");
    assert_eq!(descriptor_field_number(response, "access"), Some(1));
    assert_eq!(descriptor_field_number(response, "error"), Some(6));
    let read_request = descriptor_message(session, "ReadGrantedImageRequest");
    assert_eq!(descriptor_field_number(read_request, "grant_id"), Some(1));
    assert_eq!(descriptor_field_number(read_request, "proof"), Some(2));
    assert_eq!(descriptor_field_number(read_request, "image_id"), Some(3));
    let read_response = descriptor_message(session, "ReadGrantedImageResponse");
    assert_eq!(descriptor_field_number(read_response, "image"), Some(1));
    assert_eq!(descriptor_field_number(read_response, "error"), Some(4));
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
        ("media_transport", 9),
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
fn session_broadcast_parent_and_leg_contract_is_stable() {
    let descriptor = descriptor();
    let session = descriptor
        .file
        .iter()
        .find(|file| file.package.as_deref() == Some("gmv.session.v1"))
        .unwrap();
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

    for (name, number) in [
        ("broadcast_id", 18),
        ("broadcast_leg_id", 19),
        ("expected_stream_node_id", 20),
    ] {
        assert_eq!(field_number("StartDeviceStreamRequest", name), Some(number));
    }
    assert_eq!(
        field_number("DeviceStreamResponse", "broadcast_profile"),
        Some(12)
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
        media_transport: gmv_protocol::stream::v1::MediaTransport::TcpPassive as i32,
    };
    let decoded_legacy =
        LegacyStartReceiveRequest::decode(current.encode_to_vec().as_slice()).unwrap();
    assert_eq!(decoded_legacy.stream_id, current.stream_id);
    assert_eq!(decoded_legacy.route_id, current.route_id);
    assert_eq!(decoded_legacy.lease_id, current.lease_id);
}
