extern crate self as avai;

pub mod feedback;
pub mod guard_integration;
pub mod model;
#[path = "model/management/mod.rs"]
pub mod model_management;
pub mod observability;
pub mod source;
pub mod task;
pub mod upload;

#[cfg(test)]
#[path = "../tests/unit/feedback/mod.rs"]
mod feedback_tests;
#[cfg(test)]
#[path = "../tests/unit/model/management.rs"]
mod model_management_tests;
#[cfg(test)]
#[path = "../tests/unit/model/runtime_tests.rs"]
mod model_runtime_tests;
#[cfg(test)]
#[path = "../tests/unit/task/model_execution.rs"]
mod task_model_execution_tests;
