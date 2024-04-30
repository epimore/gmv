use common::err::GlobalResult;
use crate::biz::call::StreamState;

pub struct ResMsg<T> {
    code: i8,
    msg: String,
    data: Option<T>,
}

impl<T> ResMsg<T> {
    pub fn build_success() -> Self {
        Self { code: 0, msg: "success".to_string(), data: None }
    }
    pub fn build_failed() -> Self {
        Self { code: -1, msg: "failed".to_string(), data: None }
    }

    pub fn build_failed_by_msg(msg: String) -> Self {
        Self { code: -1, msg, data: None }
    }

    pub fn define_res(code: i8, msg: String) -> Self {
        Self { code, msg, data: None }
    }
}

//监听ssrc，返回状态
pub async fn listen_ssrc(ssrc: &String, stream_id: &String) -> GlobalResult<()> {
    unimplemented!()
}

//删除ssrc，返回正在使用的stream_id/token
pub async fn drop_ssrc(ssrc: &String) -> GlobalResult<()> {
    unimplemented!()
}

//开启录像
pub async fn start_record(ssrc: &String, file_name: &String) {}

//停止录像，是否清理录像文件
pub async fn stop_record(ssrc: &String, clean: bool) {}

//踢出用户观看
pub async fn kick_token(stream_id: &String, token: &String){}

impl ResMsg<Vec<StreamState>> {
    //查询流媒体数据状态,hls/flv/record
    pub async fn get_state(ssrc: Option<String>, stream_id: Option<String>) { unimplemented!() }
}

