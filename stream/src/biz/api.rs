use std::collections::HashMap;
use std::net::SocketAddr;

use hyper::{Body, header, Response, StatusCode};
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use common::err::{BizError, GlobalError, GlobalResult, TransError};
use common::log::{error, info};
use common::tokio;
use common::tokio::sync::{oneshot};
use common::tokio::sync::mpsc::Sender;

use crate::biz::call::{StreamPlayInfo, StreamState};
use crate::general::mode::{Media, ResMsg};
use crate::io::hook_handler::{Event, EventRes};
use crate::state::cache;
use crate::trans::flv_muxer;

fn get_ssrc(param_map: &HashMap<String, String>) -> GlobalResult<u32> {
    let ssrc = param_map.get("ssrc")
        .map(|s| s.parse::<u32>().hand_log(|msg| error!("{msg}")))
        .ok_or_else(|| GlobalError::new_biz_error(1100, "stream_id 不存在", |msg| error!("{msg}")))??;
    Ok(ssrc)
}

fn get_stream_id(param_map: &HashMap<String, String>) -> GlobalResult<String> {
    let stream_id = param_map.get("stream_id").ok_or_else(|| GlobalError::new_biz_error(1100, "stream_id 不存在", |msg| error!("{msg}")))?;
    Ok(stream_id.to_string())
}

pub fn res_401() -> GlobalResult<Response<Body>> {
    let json_data = ResMsg::<bool>::build_failed_by_msg("401 无token".to_string()).to_json()?;
    let res = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .status(StatusCode::UNAUTHORIZED).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}

pub fn res_404() -> GlobalResult<Response<Body>> {
    let json_data = ResMsg::<bool>::build_failed_by_msg("404".to_string()).to_json()?;
    let res = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .status(StatusCode::NOT_FOUND).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}
pub fn res_204() -> GlobalResult<Response<Body>> {
    let json_data = ResMsg::<String>::build_success_data("No Content;设备端结束媒体流".to_string()).to_json()?;
    let res = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .status(StatusCode::NO_CONTENT).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}

#[allow(dead_code)]
pub fn res_500(msg: &str) -> GlobalResult<Response<Body>> {
    let json_data = ResMsg::<bool>::build_failed_by_msg(msg.to_string()).to_json()?;
    let res = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .status(StatusCode::NOT_FOUND).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}

#[allow(dead_code)]
//todo 获取流超时对点播方响应
pub fn res_404_stream_timeout() -> GlobalResult<Response<Body>> {
    let json_data = ResMsg::<bool>::build_failed_by_msg("404:media stream disconnected".to_string()).to_json()?;
    let res = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .status(StatusCode::NOT_FOUND).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}

pub fn res_422() -> GlobalResult<Response<Body>> {
    let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
    let json_data = ResMsg::<bool>::build_failed_by_msg("参数错误".to_string()).to_json()?;
    let res = response.status(StatusCode::UNPROCESSABLE_ENTITY).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}

//监听ssrc，返回状态
pub async fn listen_ssrc(map: HashMap<String, String>, ssrc_tx: Sender<u32>) -> GlobalResult<Response<Body>> {
    match (get_ssrc(&map), get_stream_id(&map)) {
        (Ok(ssrc), Ok(stream_id)) => {
            let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
            let res = match cache::insert(ssrc, stream_id, cache::Channel::build()) {
                Ok(_) => {
                    ssrc_tx.send(ssrc).await.hand_log(|msg| error!("{msg}"))?;
                    let json_data = ResMsg::<bool>::build_success().to_json()?;
                    response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?
                }
                Err(error) => {
                    let json_data = ResMsg::<bool>::build_failed_by_msg(error.to_string()).to_json()?;
                    response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?
                }
            };
            Ok(res)
        }
        _ => {
            res_422()
        }
    }
}

#[derive(Deserialize, Debug)]
pub struct RtpMap {
    ssrc: u32,
    map: HashMap<u8, String>,
}

impl RtpMap {
    //指定媒体流类型映射
    pub fn rtp_map(rtp_map: RtpMap) -> GlobalResult<Response<Body>> {
        let mut map = HashMap::new();
        for (tp, val) in rtp_map.map {
            match Media::build(&val).hand_log(|msg| error!("{msg}")) {
                Ok(media) => {
                    map.insert(tp, media);
                }
                Err(_err) => {
                    return res_422();
                }
            }
        }
        let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
        let res = match cache::insert_media_type(rtp_map.ssrc, map) {
            Ok(_) => {
                let json_data = ResMsg::<bool>::build_success().to_json()?;
                response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?
            }
            Err(error) => {
                let json_data = ResMsg::<bool>::build_failed_by_msg(error.to_string()).to_json()?;
                response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?
            }
        };
        Ok(res)
    }
}

//删除ssrc，返回正在使用的stream_id/token
// pub async fn drop_ssrc(ssrc: u32) -> GlobalResult<()> {
//     unimplemented!()
// }

//开启录像
// pub async fn start_record(ssrc: u32, file_name: &String) {}

//停止录像，是否清理录像文件
// pub async fn stop_record(ssrc: u32, clean: bool) {}

//查询流媒体数据状态,hls/flv/record:ResMsg<Vec<StreamState>>
pub fn get_state(opt_stream_id: Option<String>) -> GlobalResult<Response<Body>> {
    let vec = cache::get_stream_state(opt_stream_id);
    let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
    let json_data = ResMsg::<Vec<StreamState>>::build_success_data(vec).to_json()?;
    let res = response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    Ok(res)
}

//开启播放:play_type=flv/hls
pub async fn start_play(play_type: String, stream_id: String, token: String, remote_addr: SocketAddr, client_connection_cancel: CancellationToken) -> GlobalResult<Response<Body>> {
    match cache::get_base_stream_info_by_stream_id(&stream_id) {
        None => { res_404() }
        Some((bsi, flv_tokens, hls_tokens)) => {
            let ssrc = *(bsi.get_rtp_info().get_ssrc());
            let info = StreamPlayInfo::new(bsi, remote_addr.to_string(), token.clone(), play_type.clone(), flv_tokens, hls_tokens);
            let (tx, rx) = oneshot::channel();
            let event_tx = cache::get_event_tx();
            let _ = event_tx.clone().send((Event::OnPlay(info), Some(tx))).await.hand_log(|msg| error!("{msg}"));
            match rx.await {
                Ok(res) => {
                    let res_builder = Response::builder()
                        .status(StatusCode::OK)
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Transfer-Encoding", "chunked")
                        .header("Connection", "keep-alive")
                        .header("Cache-Control", "no-cache"); //Cache-Control: 根据需求设置，一般可以设为 no-cache 或者 public, max-age=秒数。
                    if let EventRes::OnPlay(Some(true)) = res {
                        match &play_type[..] {
                            "flv" => {
                                match cache::get_flv_rx(&ssrc) {
                                    Some(rx) => {
                                        let (flv_tx, body) = Body::channel();
                                        tokio::spawn(async move {
                                            if let Err(GlobalError::BizErr(BizError { code: 1199, .. })) = flv_muxer::send_flv(ssrc, flv_tx, rx).await {
                                                return res_204();
                                            }
                                            Ok(Default::default())
                                        });

                                        let flv_res = res_builder.header("Content-Type", "video/x-flv")
                                            .body(body).hand_log(|msg| error!("{msg}"))?;
                                        //插入用户
                                        cache::update_token(&stream_id, &play_type, token.clone(), true);
                                        //监听连接：当断开连接时,更新正在查看的用户、回调通知
                                        tokio::spawn(async move {
                                            client_connection_cancel.cancelled().await;
                                            info!("HTTP 用户端断开FLV媒体流：ssrc={},stream_id={},gmv_token={}",ssrc,&stream_id,&token);
                                            cache::update_token(&stream_id, &play_type, token.clone(), false);
                                            if let Some((bsi, flv_tokens, hls_tokens)) = cache::get_base_stream_info_by_stream_id(&stream_id) {
                                                let info = StreamPlayInfo::new(bsi, remote_addr.to_string(), token, play_type, flv_tokens, hls_tokens);
                                                let _ = event_tx.send((Event::OffPlay(info), None)).await.hand_log(|msg| error!("{msg}"));
                                            }
                                        });
                                        Ok(flv_res)
                                    }
                                    _ => { res_404() }
                                }
                            }
                            "hls" => {
                                match cache::get_hls_rx(&ssrc) {
                                    None => { res_404() }
                                    Some(_rx) => {
                                        let (_hls_tx, body) = Body::channel();
                                        tokio::spawn(async {
                                            unimplemented!()
                                            // hls_process::send_hls(hls_tx, rx).await
                                        });

                                        //Content-Type：返回的数据类型;HLS M3U8,通常是 application/vnd.apple.mpegurl 或 application/x-mpegURL。
                                        let hls_res = res_builder.header("Content-Type", "application/x-mpegURL")
                                            .body(body).hand_log(|msg| error!("{msg}"))?;
                                        cache::update_token(&stream_id, &play_type, token.clone(), true);
                                        //监听连接：当断开连接时,更新正在查看的用户、回调通知
                                        tokio::spawn(async move {
                                            client_connection_cancel.cancelled().await;
                                            cache::update_token(&stream_id, &play_type, token.clone(), false);
                                            if let Some((bsi, flv_tokens, hls_tokens)) = cache::get_base_stream_info_by_stream_id(&stream_id) {
                                                let info = StreamPlayInfo::new(bsi, remote_addr.to_string(), token, play_type, flv_tokens, hls_tokens);
                                                let _ = event_tx.send((Event::OffPlay(info), None)).await.hand_log(|msg| error!("{msg}"));
                                            }
                                        });
                                        Ok(hls_res)
                                    }
                                }
                            }
                            _ => { res_422() }
                        }
                    } else {
                        res_401()
                    }
                }
                Err(_) => {
                    //对端关闭,表示流已释放
                    res_404()
                }
            }
        }
    }
}

//关闭播放，stp:0-all,1-flv,2-hls
// pub async fn stop_play(stream_id: String, token: String, stp: u8) {}