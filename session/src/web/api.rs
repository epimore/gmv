use common::log::{error, info};
use poem_openapi::OpenApi;
use poem_openapi::param::Header;
use poem_openapi::payload::{Json};

use common::exception::{GlobalError};

use crate::general::model::*;
use crate::service::{biz, handler, StreamRecordInfo};

pub struct RestApi;

#[OpenApi(prefix_path = "/api")]
impl RestApi {
    #[allow(non_snake_case)]
    #[oai(path = "/play/live/stream", method = "post")]
    /// 点播监控实时画面 transMode 默认0 udp 模式, 1 tcp 被动模式,2 tcp 主动模式
    async fn play_live(&self, live: Json<PlayLiveModel>, #[oai(
        name = "gmv-token"
    )] token: Header<String>) -> Json<ResultMessageData<StreamInfo>> {
        let header = token.0;
        let live_model = live.0;
        info!("play_live:header = {:?},body = {:?}", &header,&live_model);
        match handler::play_live(live_model, header).await {
            Ok(data) => { Json(ResultMessageData::build_success(data)) }
            Err(err) => {
                error!("{}",err.to_string());
                match err {
                    GlobalError::BizErr(e) => {
                        Json(ResultMessageData::build_failure_msg(e.msg))
                    }
                    GlobalError::SysErr(_e) => {
                        Json(ResultMessageData::build_failure())
                    }
                }
            }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/play/back/stream", method = "post")]
    /// 点播监控历史画面 transMode 默认0 udp 模式, 1 tcp 被动模式,2 tcp 主动模式
    async fn play_back(&self, back: Json<PlayBackModel>, #[oai(
        name = "gmv-token"
    )] token: Header<String>) -> Json<ResultMessageData<StreamInfo>> {
        let header = token.0;
        let back_model = back.0;
        info!("back_model:header = {:?},body = {:?}", &header,&back_model);
        match handler::play_back(back_model, header).await {
            Ok(data) => { Json(ResultMessageData::build_success(data)) }
            Err(err) => {
                error!("{}",err.to_string());
                match err {
                    GlobalError::BizErr(e) => {
                        Json(ResultMessageData::build_failure_msg(e.msg))
                    }
                    GlobalError::SysErr(_e) => {
                        Json(ResultMessageData::build_failure())
                    }
                }
            }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/play/back/seek", method = "post")]
    /// 拖动播放录像 seek 拖动秒 [1-86400]
    async fn playback_seek(&self,
                           seek: Json<PlaySeekModel>,
                           #[oai(name = "gmv-token")] token: Header<String>)
                           -> Json<ResultMessageData<bool>> {
        let header = token.0;
        let seek_model = seek.0;
        info!("back-seek:header = {:?},body = {:?}", &header,&seek_model);
        match handler::seek(seek_model, header).await {
            Err(err) => {
                let err_msg = format!("拖动失败；{}", err);
                error!("{}",&err_msg);
                Json(ResultMessageData::build_failure_msg(err_msg))
            }
            Ok(_) => { Json(ResultMessageData::build_success(true)) }
        }
    }
    #[allow(non_snake_case)]
    #[oai(path = "/play/back/speed", method = "post")]
    /// 倍速播放历史视频 speed [1,2,4]
    async fn playback_speed(&self,
                            speed: Json<PlaySpeedModel>,
                            #[oai(name = "gmv-token")] token: Header<String>)
                            -> Json<ResultMessageData<bool>> {
        let header = token.0;
        let speed_model = speed.0;
        info!("back-speed:header = {:?},body = {:?}", &header,&speed_model);
        match handler::speed(speed_model, header).await {
            Err(err) => {
                let err_msg = format!("倍速播放失败；{}", err);
                error!("{}",&err_msg);
                Json(ResultMessageData::build_failure_msg(err_msg))
            }
            Ok(_) => { Json(ResultMessageData::build_success(true)) }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/control/ptz", method = "post")]
    /// 云台控制
    async fn control_ptz(&self,
                         ptz: Json<PtzControlModel>,
                         #[oai(name = "gmv-token")] token: Header<String>)
                         -> Json<ResultMessageData<bool>> {
        let header = token.0;
        let ptz_model = ptz.0;
        info!("control_ptz:header = {:?},body = {:?}", &header,&ptz_model);
        match handler::ptz(ptz_model, header).await {
            Err(err) => {
                let err_msg = format!("云台控制失败；{}", err);
                error!("{}",&err_msg);
                Json(ResultMessageData::build_failure_msg(err_msg))
            }
            Ok(_) => { Json(ResultMessageData::build_success(true)) }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/download/mp4", method = "post")]
    /// 开始录像
    async fn download(&self, back: Json<PlayBackModel>,
                      #[oai(name = "gmv-token")] token: Header<String>)
                      -> Json<ResultMessageData<String>> {
        let header = token.0;
        let back_model = back.0;
        info!("download:header = {:?},body = {:?}", &header,&back_model);
        match handler::download(back_model, header).await {
            Ok(stream_id) => { Json(ResultMessageData::build_success(stream_id)) }
            Err(err) => {
                error!("{}",err.to_string());
                match err {
                    GlobalError::BizErr(e) => {
                        Json(ResultMessageData::build_failure_msg(e.msg))
                    }
                    GlobalError::SysErr(_e) => {
                        Json(ResultMessageData::build_failure())
                    }
                }
            }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/download/stop", method = "post")]
    /// 提前终止云端录像任务
    async fn download_stop(&self,
                           stream_id: Json<String>,
                           #[oai(
                               name = "gmv-token"
                           )] token: Header<String>) -> Json<ResultMessageData<bool>> {
        let header = token.0;
        let stream_id = stream_id.0;
        info!("teardown:header = {:?},body = {:?}", &header,&stream_id);
        match handler::download_stop(stream_id, header).await {
            Err(err) => {
                error!("终止失败；{}",err);
                Json(ResultMessageData::build_failure())
            }
            Ok(_info) => { Json(ResultMessageData::build_success(true)) }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/downing/info", method = "post")]
    /// 查看进行中录像信息
    async fn down_info(&self,
                       stream_node: Json<StreamNode>,
                       #[oai(
                           name = "gmv-token"
                       )] token: Header<String>) -> Json<ResultMessageData<StreamRecordInfo>> {
        let header = token.0;
        let stream_node = stream_node.0;
        let stream_id = stream_node.stream_id;
        let stream_server = stream_node.stream_server;
        info!("down_info:header = {:?},body = {:?},{:?}", &header,&stream_id,&stream_server);
        match handler::download_info_by_stream_id(stream_id, stream_server, header).await {
            Err(err) => {
                error!("查看录像信息失败；{}",err);
                Json(ResultMessageData::build_failure())
            }
            Ok(info) => { Json(ResultMessageData::build_success(info)) }
        }
    }

    #[allow(non_snake_case)]
    #[oai(path = "/rm/file", method = "post")]
    /// 物理删除文件
    async fn rm_file(&self,
                     file_id: Json<String>,
                     #[oai(
                         name = "gmv-token"
                     )] token: Header<String>) -> Json<ResultMessageData<bool>> {
        let header = token.0;
        let file_id = file_id.0;
        info!("rm_file:header = {:?},body = {:?}", &header,&file_id);
        match biz::rm_file(file_id).await {
            Err(err) => {
                error!("删除失败；{}",err);
                Json(ResultMessageData::build_failure())
            }
            Ok(_) => { Json(ResultMessageData::build_success(true)) }
        }
    }
}