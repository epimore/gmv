extern crate self as avai;

pub mod guard_integration;
pub mod model;
pub mod model_management;
pub mod observability;
pub mod source;
pub mod task;
pub mod upload;

#[cfg(test)]
mod model_management_tests;
#[cfg(test)]
#[path = "../tests/model_runtime.rs"]
mod model_runtime_tests;
#[cfg(test)]
#[path = "../tests/task_model_execution.rs"]
mod task_model_execution_tests;
