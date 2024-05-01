use std::collections::HashMap;
use std::time::Duration;

use hyper::{Body, header, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};

use common::err::{GlobalError, GlobalResult, TransError};
use common::log::error;
use common::tokio::sync::mpsc::Sender;

use crate::biz::call::StreamState;
use crate::general::mode::{ResMsg, TIME_OUT};
use crate::state::cache;

fn get_ssrc(param_map: &HashMap<String, String>) -> GlobalResult<u32> {
    let ssrc = param_map.get("ssrc")
        .map(|s| s.parse::<u32>().hand_err(|msg| error!("{msg}")))
        .ok_or(GlobalError::new_biz_error(1100, "stream_id 不存在", |msg| error!("{msg}")))??;
    Ok(ssrc)
}

fn get_stream_id(param_map: &HashMap<String, String>) -> GlobalResult<String> {
    let stream_id = param_map.get("stream_id").ok_or(GlobalError::new_biz_error(1100, "stream_id 不存在", |msg| error!("{msg}")))?;
    Ok(stream_id.to_string())
}

fn get_param_map(req: &Request<Body>) -> GlobalResult<HashMap<String, String>> {
    let map = form_urlencoded::parse(req.uri().query()
        .ok_or(GlobalError::new_biz_error(1100, "URL上参数不存在", |msg| error!("{msg}")))?.as_bytes())
        .into_owned()
        .collect::<HashMap<String, String>>();
    Ok(map)
}

//监听ssrc，返回状态
pub async fn listen_ssrc(req: &Request<Body>, tx: Sender<u32>) -> GlobalResult<Response<Body>> {
    let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
    let param_map = get_param_map(req);
    if param_map.is_err() {
        let json_data = ResMsg::<bool>::build_failed_by_msg("参数错误".to_string()).to_json()?;
        let res = response.status(StatusCode::UNPROCESSABLE_ENTITY).body(Body::from(json_data)).hand_err(|msg| error!("{msg}"))?;
        return Ok(res);
    }
    let map = param_map?;
    let ssrc_res = get_ssrc(&map);
    let stream_id_res = get_stream_id(&map);
    if ssrc_res.is_err() || stream_id_res.is_err() {
        let json_data = ResMsg::<bool>::build_failed_by_msg("参数错误".to_string()).to_json()?;
        let res = response.status(StatusCode::UNPROCESSABLE_ENTITY).body(Body::from(json_data)).hand_err(|msg| error!("{msg}"))?;
        return Ok(res);
    }
    let ssrc = ssrc_res?;
    let res = match cache::insert(ssrc, stream_id_res?, Duration::from_millis(TIME_OUT), cache::Channel::build()) {
        Ok(_) => {
            tx.send(ssrc).await.hand_err(|msg| error!("{msg}"))?;
            let json_data = ResMsg::<bool>::build_success().to_json()?;
            response.status(StatusCode::OK).body(Body::from(json_data)).hand_err(|msg| error!("{msg}"))?
        }
        Err(error) => {
            let json_data = ResMsg::<bool>::build_failed_by_msg(error.to_string()).to_json()?;
            response.status(StatusCode::OK).body(Body::from(json_data)).hand_err(|msg| error!("{msg}"))?
        }
    };
    Ok(res)
}

//删除ssrc，返回正在使用的stream_id/token
pub async fn drop_ssrc(ssrc: u32) -> GlobalResult<()> {
    unimplemented!()
}

//开启录像
pub async fn start_record(ssrc: u32, file_name: &String) {}

//停止录像，是否清理录像文件
pub async fn stop_record(ssrc: u32, clean: bool) {}

//踢出用户观看
pub async fn kick_token(stream_id: &String, token: &String) {}

//查询流媒体数据状态,hls/flv/record:ResMsg<Vec<StreamState>>
pub async fn get_state(ssrc: Option<u32>, stream_id: Option<String>) { unimplemented!() }

//播放flv
pub async fn play_flv(stream_id: &String, token: &String) {}

//播放hls
pub async fn play_hls(stream_id: &String, token: &String) {}

