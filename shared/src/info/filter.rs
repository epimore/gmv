use base::serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(crate = "base::serde")]
pub struct Capture {}
#[derive(Serialize, Deserialize, Debug, Default)]
#[serde(crate = "base::serde")]
pub struct Filter {
    //抽图
    pub capture: Option<Capture>,
    //缩放
    // pub scale: Option<Scale>,
    //裁剪
    // pub crop: Option<Crop>,
    //旋转
    // pub rotate: Option<Rotate>,
    //镜像
    // pub mirror: Option<Mirror>,
}