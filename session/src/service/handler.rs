use std::time::Duration;

use log::error;
use mysql::serde_json;

use common::bytes::Bytes;
use common::err::{GlobalResult, TransError};
use common::tokio::sync::mpsc;
use common::tokio::sync::mpsc::Sender;
use common::tokio::time::Instant;

use crate::general;
use crate::general::model::{PlayLiveModel, StreamInfo};
use crate::service::{BaseStreamInfo, callback};

const KEY_STREAM_IN: &str = "KEY_STREAM_IN:";

pub async fn on_play() {}

pub async fn off_play() {}

pub async fn stream_in(base_stream_info: BaseStreamInfo) {
    let key_stream_in_id = format!("{KEY_STREAM_IN}{}", base_stream_info.get_stream_id());
    if let Some((_, Some(tx))) = general::cache::Cache::state_get(&key_stream_in_id).await {
        let vec = serde_json::to_vec(&base_stream_info).unwrap();
        let bytes = Bytes::from(vec);
        let _ = tx.send(Some(bytes)).await.hand_err(|msg| error!("{msg}"));
    }
    
}

pub async fn stream_input_timeout() {}

pub async fn play_live(play_live_model: PlayLiveModel, token: String) -> Option<StreamInfo> {
    let device_id = play_live_model.get_deviceId();
    let channel_id = play_live_model.get_channelId();
    //查看直播流是否已存在,有则直接返回
    if let Some((stream_id, node_name)) = enable_live_stream(device_id, channel_id, &token).await {
        general::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
        return StreamInfo::build(stream_id, node_name);
    }

    unimplemented!()
}


//首先查看session缓存中是否有映射关系,然后看stream中是否有相应数据:都为true时返回数据
//当session有,stream无时：session调用stream->使其重新监听ssrc
//(避免stream重启后,数据不一致)
async fn enable_live_stream(device_id: &String, channel_id: &String, token: &String) -> Option<(String, String)> {
    match general::cache::Cache::device_map_get_live_info(device_id, channel_id) {
        None => { None }
        //session -> true
        Some((stream_id, ssrc)) => {
            let mut res = None;
            if let Some(node_name) = general::cache::Cache::stream_map_query_node_name(&stream_id) {
                //确认stream是否存在
                if let Some(stream_node) = general::StreamConf::get_session_conf_by_cache().get_node_map().get(&node_name) {
                    if let Ok(vec) = callback::call_stream_state(Some(&stream_id), token, stream_node.get_local_ip(), stream_node.get_local_port()).await {
                        if vec.len() == 0 {
                            //session有流信息,stream无流存在=>进一步判断可能是stream重启导致没有该监听,重启监听等待结果
                            if let Ok(true) = callback::call_listen_ssrc(&stream_id, &ssrc, token, stream_node.get_local_ip(), stream_node.get_local_port()).await {
                                if let Some(_) = listen_stream_by_stream_id(&stream_id, 2).await {
                                    res = Some((stream_id.clone(), node_name));
                                }
                            }
                        } else {
                            //stream -> true
                            res = Some((stream_id.clone(), node_name));
                        }
                    }
                }
            }
            //stream中无stream_id映射,同步剔除session中映射
            if res.is_none() {
                general::cache::Cache::device_map_remove(device_id, None);
                general::cache::Cache::stream_map_remove(&stream_id, None);
            }
            res
        }
    }
}


async fn listen_stream_by_stream_id(stream_id: &String, secs: u64) -> Option<BaseStreamInfo> {
    let (tx, mut rx) = mpsc::channel(1);
    let when = Instant::now() + Duration::from_secs(secs);
    let key = format!("{KEY_STREAM_IN}{stream_id}");
    general::cache::Cache::state_insert(key.clone(), Bytes::new(), Some(when), Some(tx)).await;
    let mut res = None;
    if let Some(Some(bytes)) = rx.recv().await {
        res = serde_json::from_slice::<BaseStreamInfo>(&*bytes).ok();
    }
    general::cache::Cache::state_remove(&key).await;
    res
}

