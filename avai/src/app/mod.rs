mod config;
mod runtime;

use std::net::TcpListener;
#[cfg(test)]
use std::path::PathBuf;

use avai::feedback::FeedbackConfig;
use base::exception::GlobalError;

#[cfg(test)]
use config::validate_feedback_root;
use config::{GuardConf, ModelConf, ServerConf};

pub struct App {
    guard: GuardConf,
    server: ServerConf,
    model: ModelConf,
    feedback: FeedbackConfig,
}

pub struct Bootstrap {
    grpc_listener: TcpListener,
    upload_listener: TcpListener,
}

fn config_error(error: base::cfg_lib::conf::ConfigError) -> GlobalError {
    GlobalError::from_external_error(error, |_| {})
}

fn external_error<E>(error: E) -> GlobalError
where
    E: std::error::Error + Send + Sync + 'static,
{
    GlobalError::from_external_error(error, |_| {})
}

fn global_error(message: &str) -> GlobalError {
    GlobalError::new_sys_error(message, |_| {})
}

#[cfg(test)]
#[path = "../../tests/unit/app/feedback_root.rs"]
mod feedback_root_tests;
