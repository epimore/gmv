use common::serde::{Deserialize, Serialize};
use poem_openapi::Object;

pub mod api;
pub mod hook;
pub mod se;


#[derive(Object, Serialize, Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct SingleParam<T: poem_openapi::types::Type + poem_openapi::types::ParseFromJSON + poem_openapi::types::ToJSON> {
    pub param: T,
}