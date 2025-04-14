use common::serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Debug, Default,Copy, Clone)]
#[serde(crate = "common::serde")]
pub struct HlsPiece {
    //片时间长度 S
    pub duration: u8,
    pub live: bool,
}