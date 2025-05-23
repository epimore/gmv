use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Duration;

use hyper::{Body, header, Response, StatusCode};
use common::serde::{Deserialize};
use tokio_util::sync::CancellationToken;

use common::exception::{BizError, GlobalError, GlobalResult, TransError};
use common::log::{error, info};
use common::tokio;
use common::tokio::sync::{oneshot};
use common::tokio::sync::mpsc::Sender;
use common::tokio::time::timeout;

use crate::biz::call::{StreamPlayInfo, StreamRecordInfo};
use crate::container::PlayType;
use crate::general::mode::{Media, ResMsg, TIME_OUT};
use crate::io::hook_handler::{MediaAction, OutEvent, OutEventRes};
use crate::state::cache;
use crate::trans::flv_muxer;

#[allow(dead_code)]
pub fn get_ssrc(param_map: &HashMap<String, String>) -> GlobalResult<u32> {
    let ssrc = param_map.get("ssrc")
        .map(|s| s.parse::<u32>().hand_log(|msg| error!("{msg}")))
        .ok_or_else(|| GlobalError::new_biz_error(1100, "ssrc 不存在", |msg| error!("{msg}")))??;
    Ok(ssrc)
}


pub fn get_stream_id(param_map: &HashMap<String, String>) -> GlobalResult<String> {
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

pub fn res_400() -> GlobalResult<Response<Body>> {
    let json_data = ResMsg::<bool>::build_failed_by_msg("400".to_string()).to_json()?;
    let res = Response::builder()
        .header(header::CONTENT_TYPE, "application/json")
        .status(StatusCode::BAD_REQUEST).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
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
        .status(StatusCode::INTERNAL_SERVER_ERROR).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    return Ok(res);
}

#[allow(dead_code)]
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

//监听ssrc,接收流放入通道缓存
pub fn listen_ssrc(ssrc_lis: SsrcLisDto) -> GlobalResult<Response<Body>> {
    let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
    let res = match cache::insert(ssrc_lis) {
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

#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct SsrcLisDto {
    pub ssrc: u32,
    pub stream_id: String,
    //当为None时，默认配置,负数-立即关闭
    pub expires: Option<i32>,
    pub media_action: MediaAction,
    // pub flv: Option<bool>,
    // pub hls: Option<HlsPiece>,
}

#[derive(Deserialize, Debug)]
#[serde(crate = "common::serde")]
pub struct RtpMap {
    ssrc: u32,
    map: HashMap<u8, String>,
}

impl RtpMap {
    //指定媒体流类型映射,发送事件，消费流进行转换
    pub async fn rtp_map(rtp_map: RtpMap, ssrc_tx: Sender<u32>) -> GlobalResult<Response<Body>> {
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
                ssrc_tx.send(rtp_map.ssrc).await.hand_log(|msg| error!("{msg}"))?;
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

//媒体流录制过程回调
pub async fn on_record(stream_id: &String) -> GlobalResult<Response<Body>> {
    match cache::get_down_rx_by_stream_id(stream_id) {
        None => {
            res_404()
        }
        Some(mut rx) => {
            match timeout(Duration::from_millis(TIME_OUT), rx.recv()).await {
                Ok(res) => {
                    match res {
                        Ok(info) => {
                            let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
                            let json_data = ResMsg::<StreamRecordInfo>::build_success_data(info).to_json()?;
                            let res = response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
                            Ok(res)
                        }
                        Err(_) => {
                            res_404_stream_timeout()
                        }
                    }
                }
                Err(_) => {
                    res_404_stream_timeout()
                }
            }
        }
    }
}

//删除ssrc，返回正在使用的stream_id/token
// pub async fn drop_ssrc(ssrc: u32) -> GlobalResult<()> {
//     unimplemented!()
// }

//查询流媒体数据状态,hls/flv/record:ResMsg<Vec<StreamState>>
pub fn get_stream_count(opt_stream_id: Option<&String>) -> GlobalResult<Response<Body>> {
    let count = cache::get_stream_count(opt_stream_id);
    let response = Response::builder().header(header::CONTENT_TYPE, "application/json");
    let json_data = ResMsg::<u32>::build_success_data(count).to_json()?;
    let res = response.status(StatusCode::OK).body(Body::from(json_data)).hand_log(|msg| error!("{msg}"))?;
    Ok(res)
}

//开启播放:play_type=flv/hls
pub async fn start_play(play_type: PlayType, stream_id: String, token: String, remote_addr: SocketAddr, client_connection_cancel: CancellationToken) -> GlobalResult<Response<Body>> {
    match cache::get_base_stream_info_by_stream_id(&stream_id) {
        None => { res_404() }
        Some((bsi, user_count)) => {
            let ssrc = *(bsi.get_rtp_info().get_ssrc());
            let info = StreamPlayInfo::new(bsi, remote_addr.to_string(), token.clone(), play_type, user_count);
            let (tx, rx) = oneshot::channel();
            let event_tx = cache::get_event_tx();
            let _ = event_tx.clone().send((OutEvent::OnPlay(info), Some(tx))).await.hand_log(|msg| error!("{msg}"));
            match rx.await {
                Ok(res) => {
                    let res_builder = Response::builder()
                        .status(StatusCode::OK)
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Transfer-Encoding", "chunked")
                        .header("Connection", "keep-alive")
                        .header("Cache-Control", "no-cache"); //Cache-Control: 根据需求设置，一般可以设为 no-cache 或者 public, max-age=秒数。
                    if let OutEventRes::OnPlay(Some(true)) = res {
                        match play_type {
                            PlayType::Flv => {
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
                                        cache::update_token(&stream_id, play_type, token.clone(), true, remote_addr);
                                        //监听连接：当断开连接时,更新正在查看的用户、回调通知
                                        tokio::spawn(async move {
                                            client_connection_cancel.cancelled().await;
                                            info!("HTTP 用户端断开FLV媒体流：ssrc={},stream_id={},gmv_token={}",ssrc,&stream_id,&token);
                                            cache::update_token(&stream_id, play_type, token.clone(), false, remote_addr);
                                            if let Some((bsi, user_count)) = cache::get_base_stream_info_by_stream_id(&stream_id) {
                                                let info = StreamPlayInfo::new(bsi, remote_addr.to_string(), token, play_type, user_count);
                                                let _ = event_tx.send((OutEvent::OffPlay(info), None)).await.hand_log(|msg| error!("{msg}"));
                                            }
                                        });
                                        Ok(flv_res)
                                    }
                                    _ => { res_404() }
                                }
                            }
                            PlayType::Hls => {
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
                                        cache::update_token(&stream_id, play_type, token.clone(), true, remote_addr);
                                        //监听连接：当断开连接时,更新正在查看的用户、回调通知
                                        tokio::spawn(async move {
                                            client_connection_cancel.cancelled().await;
                                            cache::update_token(&stream_id, play_type, token.clone(), false, remote_addr);
                                            if let Some((bsi, user_count)) = cache::get_base_stream_info_by_stream_id(&stream_id) {
                                                let info = StreamPlayInfo::new(bsi, remote_addr.to_string(), token, play_type, user_count);
                                                let _ = event_tx.send((OutEvent::OffPlay(info), None)).await.hand_log(|msg| error!("{msg}"));
                                            }
                                        });
                                        Ok(hls_res)
                                    }
                                }
                            }
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