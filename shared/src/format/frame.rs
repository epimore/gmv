use common::serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct Frame {}