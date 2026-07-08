use std::collections::HashMap;

use base::err::{BaseErrorCode, ErrorOutput, error_output, global_error_output};
use base::exception::GlobalError;
use gmv_protocol::common::v1::ErrorDetail;
use tonic::metadata::{Ascii, MetadataKey, MetadataMap, MetadataValue};

pub const META_GLOBAL_CODE: &str = "global_code";
pub const META_GLOBAL_CODE_NAME: &str = "global_code_name";
pub const META_RETRYABLE: &str = "retryable";
pub const META_TONIC_GLOBAL_CODE: &str = "gmv-global-code";

#[must_use]
pub fn error_detail(code: impl Into<String>, message: impl Into<String>) -> ErrorDetail {
    ErrorDetail {
        code: code.into(),
        message: message.into(),
        metadata: HashMap::new(),
    }
}

#[must_use]
pub fn global_error_detail(code: impl Into<String>, error: &GlobalError) -> ErrorDetail {
    let output = global_error_output(error);
    let mut detail = error_detail(code, error.to_string());
    detail
        .metadata
        .insert(META_GLOBAL_CODE.to_string(), output.code.to_string());
    detail.metadata.insert(
        META_GLOBAL_CODE_NAME.to_string(),
        output.code_name.into_owned(),
    );
    detail
        .metadata
        .insert(META_RETRYABLE.to_string(), output.retryable.to_string());
    detail
}

pub fn global_error_from_detail<O>(
    detail: ErrorDetail,
    fallback_code: u16,
    context: &str,
    op: O,
) -> GlobalError
where
    O: FnOnce(&str),
{
    let code = detail
        .metadata
        .get(META_GLOBAL_CODE)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(fallback_code);
    let detail_code = detail.code;
    let message = if detail.message.trim().is_empty() {
        if detail_code.trim().is_empty() {
            "remote error".to_string()
        } else {
            detail_code.clone()
        }
    } else {
        detail.message
    };
    GlobalError::new_biz_error(code, &message, |args| {
        let log_line = format!("{context}: {args}; error_detail_code={detail_code}");
        op(&log_line);
    })
}

pub fn global_error_from_tonic_status<O>(status: tonic::Status, context: &str, op: O) -> GlobalError
where
    O: FnOnce(&str),
{
    let status_code = status.code();
    let message = if status.message().trim().is_empty() {
        status_code.to_string()
    } else {
        status.message().to_string()
    };
    let code = metadata_value(status.metadata(), META_TONIC_GLOBAL_CODE)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or_else(|| base_code_for_tonic_status(status_code));
    GlobalError::new_biz_error(code, &message, |args| {
        let log_line = format!("{context}: {args}; tonic_code={status_code:?}");
        op(&log_line);
    })
}

#[must_use]
pub fn global_error_status(error: &GlobalError) -> tonic::Status {
    let output = global_error_output(error);
    let mut metadata = MetadataMap::new();
    insert_metadata(
        &mut metadata,
        META_TONIC_GLOBAL_CODE,
        output.code.to_string(),
    );
    tonic::Status::with_metadata(
        tonic_code_for_global_error(error, output.code),
        error.to_string(),
        metadata,
    )
}

pub fn global_error_output_from_tonic_status(status: &tonic::Status) -> Option<(u16, ErrorOutput)> {
    let code = metadata_value(status.metadata(), META_TONIC_GLOBAL_CODE)?
        .parse()
        .ok()?;
    error_output(code).map(|output| (code, output))
}

fn base_code_for_tonic_status(code: tonic::Code) -> u16 {
    match code {
        tonic::Code::DeadlineExceeded => BaseErrorCode::Timeout.code(),
        tonic::Code::Unavailable => BaseErrorCode::Network.code(),
        tonic::Code::ResourceExhausted => BaseErrorCode::IoBusy.code(),
        tonic::Code::NotFound => BaseErrorCode::NotFound.code(),
        tonic::Code::InvalidArgument => BaseErrorCode::InvalidRequest.code(),
        tonic::Code::AlreadyExists => BaseErrorCode::AlreadyExists.code(),
        tonic::Code::FailedPrecondition | tonic::Code::OutOfRange => {
            BaseErrorCode::InvalidState.code()
        }
        tonic::Code::Unauthenticated => BaseErrorCode::Unauthorized.code(),
        tonic::Code::PermissionDenied => BaseErrorCode::PermissionDenied.code(),
        tonic::Code::Unimplemented => BaseErrorCode::Unsupported.code(),
        _ => BaseErrorCode::Internal.code(),
    }
}

fn tonic_code_for_global_error(error: &GlobalError, code: u16) -> tonic::Code {
    if matches!(error, GlobalError::SysErr(_)) {
        return tonic::Code::Internal;
    }
    match code {
        1140 => tonic::Code::NotFound,
        1150 => tonic::Code::InvalidArgument,
        1160 => tonic::Code::AlreadyExists,
        1170 => tonic::Code::Unauthenticated,
        1180 => tonic::Code::PermissionDenied,
        1190 => tonic::Code::FailedPrecondition,
        1200 => tonic::Code::Unimplemented,
        1210 => tonic::Code::DeadlineExceeded,
        1220 => tonic::Code::Unavailable,
        1230 => tonic::Code::ResourceExhausted,
        _ => tonic::Code::FailedPrecondition,
    }
}

fn insert_metadata(metadata: &mut MetadataMap, key: &'static str, value: impl AsRef<str>) {
    if let Ok(value) = MetadataValue::try_from(value.as_ref()) {
        metadata.insert(MetadataKey::<Ascii>::from_static(key), value);
    }
}

fn metadata_value<'a>(metadata: &'a MetadataMap, key: &'static str) -> Option<&'a str> {
    metadata
        .get(MetadataKey::<Ascii>::from_static(key))
        .and_then(|value| value.to_str().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_error_detail_carries_registered_metadata() {
        let error =
            GlobalError::new_biz_error(BaseErrorCode::Timeout.code(), "rpc timeout", |_| {});
        let detail = global_error_detail("node_rpc_timeout", &error);

        assert_eq!(detail.code, "node_rpc_timeout");
        assert_eq!(detail.message, error.to_string());
        assert_eq!(
            detail.metadata.get(META_GLOBAL_CODE).map(String::as_str),
            Some("1210")
        );
        assert_eq!(
            detail
                .metadata
                .get(META_GLOBAL_CODE_NAME)
                .map(String::as_str),
            Some("Timeout")
        );
        assert_eq!(
            detail.metadata.get(META_RETRYABLE).map(String::as_str),
            Some("true")
        );
    }

    #[test]
    fn detail_to_global_error_prefers_metadata_code() {
        let mut detail = error_detail("stream_control_failed", "stream rejected");
        detail
            .metadata
            .insert(META_GLOBAL_CODE.to_string(), "1210".to_string());
        let mut logs = Vec::new();
        let error = global_error_from_detail(
            detail,
            BaseErrorCode::Internal.code(),
            "stream rpc",
            |msg| {
                logs.push(msg.to_string());
            },
        );

        let GlobalError::BizErr(error) = error else {
            panic!("expected biz error");
        };
        assert_eq!(error.code, BaseErrorCode::Timeout.code());
        assert_eq!(error.msg, "stream rejected");
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn tonic_status_maps_to_base_error_code() {
        let mut logs = Vec::new();
        let error = global_error_from_tonic_status(
            tonic::Status::deadline_exceeded("deadline"),
            "stream rpc",
            |msg| logs.push(msg.to_string()),
        );

        let GlobalError::BizErr(error) = error else {
            panic!("expected biz error");
        };
        assert_eq!(error.code, BaseErrorCode::Timeout.code());
        assert_eq!(error.msg, "deadline");
        assert_eq!(logs.len(), 1);
    }

    #[test]
    fn global_error_status_carries_metadata() {
        let error = GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "invalid storage query",
            |_| {},
        );
        let status = global_error_status(&error);

        assert_eq!(status.code(), tonic::Code::InvalidArgument);
        assert_eq!(
            status
                .metadata()
                .get(META_TONIC_GLOBAL_CODE)
                .and_then(|value| value.to_str().ok()),
            Some("1150")
        );
        assert_eq!(
            global_error_output_from_tonic_status(&status)
                .map(|(code, output)| { (code, output.code_name.into_owned(), output.retryable) }),
            Some((1150, "InvalidRequest".to_string(), false))
        );
    }

    #[test]
    fn tonic_status_metadata_preferred_when_rebuilding_global_error() {
        let source = GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "channel state invalid",
            |_| {},
        );
        let status = global_error_status(&source);
        let rebuilt = global_error_from_tonic_status(status, "storage rpc", |_| {});

        let GlobalError::BizErr(error) = rebuilt else {
            panic!("expected biz error");
        };
        assert_eq!(error.code, BaseErrorCode::InvalidState.code());
        assert_eq!(error.msg, source.to_string());
    }
}
