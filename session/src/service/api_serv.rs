use std::fs;
use std::ops::Sub;
use std::path::Path;
use std::time::Duration;
use pretend::Json;
use base::bytes::Bytes;
use base::chrono::{Local, TimeZone};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error};
use base::serde_json;
use base::tokio::sync::mpsc;
use base::tokio::time::{Instant, sleep};
use shared::info::format::{Flv, Mp4};
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::{MediaExt, MediaMap, MediaType};
use shared::info::obj::{BaseStreamInfo, StreamKey, StreamPlayInfo, StreamRecordInfo, StreamState};
use shared::info::output1;
use shared::info::output::{HttpFlvOutput, LocalMp4Output, OutputKind};
use shared::info::res::Resp;
use crate::gb::handler::cmd::{CmdControl, CmdStream};
use crate::gb::RWSession;
use crate::state;
use crate::state::cache::AccessMode;
use crate::state::{DownloadConf, StreamConf};
use crate::state::model::{CustomMediaConfig, PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel, PtzControlModel, SingleParam, StreamInfo, StreamMode, TransMode};
use crate::service::{KEY_STREAM_IN, RELOAD_EXPIRES};
use crate::storage::entity::{GmvFileInfo, GmvRecord};
use crate::utils::{id_builder};
use crate::http::client::{HttpClient, HttpStream};

/*
1.检查设备状态：是否在线
2.判断通道是否为单IPC
3.开启直播流
4.建立流与用户关系
*/
pub async fn play_live(play_live_model: PlayLiveModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = &play_live_model.device_id;
    if !RWSession::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| error!("{msg}")));
    }
    let channel_id = if let Some(channel_id) = &play_live_model.channel_id {
        channel_id
    } else {
        device_id
    };
    let am = AccessMode::Live;
    //查看直播流是否已存在,有则直接返回
    if let Some((stream_id, node_name)) = enable_invite_stream(device_id, channel_id, &am).await? {
        state::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
        return Ok(StreamInfo::build(stream_id, node_name));
    }
    let (stream_id, node_name) = start_invite_stream(device_id, channel_id, &token, am, 0, 0, play_live_model.trans_mode, play_live_model.custom_media_config).await?;
    state::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
    Ok(StreamInfo::build(stream_id, node_name))
}

pub async fn download_info_by_stream_id(stream_id: String, stream_server: String, _token: String) -> GlobalResult<StreamRecordInfo> {
    let conf = StreamConf::get_stream_conf();
    match conf.node_map.get(&stream_server) {
        None => { Err(GlobalError::new_biz_error(1100, "stream_server 错误", |msg| error!("{msg}"))) }
        Some(node) => {
            let p = HttpClient::template_ip_port(&node.local_ip.to_string(), node.local_port)?;
            let json_obj = p.record_info(&SingleParam { param: stream_id }).await.hand_log(|msg| error!("{msg}"))?;
            let value = json_obj.value();
            if value.code == 200 {
                match value.data {
                    None => {
                        Err(GlobalError::new_biz_error(1100, "stream_server 错误", |msg| error!("{msg}: {}", &value.msg)))
                    }
                    Some(info) => {
                        Ok(info)
                    }
                }
            } else {
                Err(GlobalError::new_biz_error(1100, "stream_server 错误", |msg| error!("{msg}: {}", &value.msg)))
            }
        }
    }
}

//todo stream -> stop api
pub async fn download_stop(stream_id: String, _token: String) -> GlobalResult<bool> {
    unimplemented!();
    /*let cst_info = state::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
    if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
        if let Ok(Some(mut record)) = GmvRecord::query_gmv_record_by_biz_id(&stream_id).await {
            record.state = 3;
            record.lt = Local::now().naive_local();
            record.update_gmv_record_by_biz_id().await?;
        }
        if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
            let _ = CmdStream::play_bye(seq, call_id, &device_id, &channel_id, &from_tag, &to_tag).await;
        }
        if let Some(am) = state::cache::Cache::stream_map_query_play_type_by_stream_id(&stream_id) {
            state::cache::Cache::device_map_remove(&device_id, Some((&channel_id, Some((am, &ssrc_str)))));
            state::cache::Cache::stream_map_remove(&stream_id, None);
        }
        let ssrc = u32::from_str_radix(&ssrc_str, 10).hand_log(|msg| error!("{msg}"))?;
        let ssrc_num = (ssrc % 10000) as u16;
        state::cache::Cache::ssrc_sn_set(ssrc_num);
        return Ok(true);
    }
    Ok(false)*/
}

pub async fn download(play_back_model: PlayBackModel, token: String) -> GlobalResult<String> {
    let device_id = &play_back_model.device_id;
    if !RWSession::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| error!("{msg}")));
    }
    let channel_id = if let Some(channel_id) = &play_back_model.channel_id {
        channel_id
    } else {
        device_id
    };
    let am = AccessMode::Down;
    //查看是否有任务
    if let Some(_) = GmvRecord::query_gmv_record_run_by_device_id_channel_id(device_id, channel_id).await? {
        return Err(GlobalError::new_biz_error(1000, "任务已存在", |msg| error!("{msg}")));
    }
    let st = play_back_model.st;
    let et = play_back_model.et;

    let storage_path = DownloadConf::get_download_conf().storage_path;
    let date_str = Local::now().format("%Y%m%d").to_string();
    let path = Path::new(&storage_path).join(date_str);
    fs::create_dir_all(&path).hand_log(|msg| error!("{msg}"))?;
    let abs_path = path.canonicalize().hand_log(|msg| error!("{msg}"))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();

    let down_conf = (&play_back_model).custom_media_config.clone().unwrap_or_else(|| {
        CustomMediaConfig{
            output: OutputKind::LocalMp4(LocalMp4Output{fmt:Mp4::default(),path:abs_path.clone()}),
            codec: None,
            filter: Default::default(),
        }
    });
    let (stream_id, node_name) = start_invite_stream(device_id, channel_id, &token, am, st - 2, et + 1, play_back_model.trans_mode, Some(down_conf)).await?;
    state::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
    let record = GmvRecord {
        biz_id: stream_id.clone(),
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        user_id: None,
        st: Local.timestamp_opt(st as i64, 0).unwrap().naive_local(),
        et: Local.timestamp_opt(et as i64, 0).unwrap().naive_local(),
        speed: 1,
        ct: Local::now().naive_local(),
        state: 0,
        lt: Local::now().naive_local(),
        stream_app_name: node_name,
    };
    record.insert_single_gmv_record().await?;
    Ok(stream_id)
}

pub async fn play_back(play_back_model: PlayBackModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = &play_back_model.device_id;
    if !RWSession::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| error!("{msg}")));
    }
    let channel_id = if let Some(channel_id) = &play_back_model.channel_id {
        channel_id
    } else {
        device_id
    };
    let am = AccessMode::Back;
    //查看流是否已存在,有则直接返回
    if let Some((stream_id, node_name)) = enable_invite_stream(device_id, channel_id, &am).await? {
        state::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
        return Ok(StreamInfo::build(stream_id, node_name));
    }
    let st = play_back_model.st;
    let et = play_back_model.et;
    let (stream_id, node_name) = start_invite_stream(device_id, channel_id, &token, am, st, et, play_back_model.trans_mode, play_back_model.custom_media_config).await?;
    state::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
    Ok(StreamInfo::build(stream_id, node_name))
}

pub async fn seek(seek_mode: PlaySeekModel, _token: String) -> GlobalResult<bool> {
    let (device_id, channel_id, _ssrc) = id_builder::de_stream_id(&*seek_mode.streamId)?;
    let (call_id, seq, from_tag, to_tag) = state::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&seek_mode.streamId)
        .ok_or_else(|| GlobalError::new_biz_error(1100, "流不存在", |msg| error!("{msg}")))?;
    CmdStream::play_seek(&device_id, &channel_id, seek_mode.seekSecond, &from_tag, &to_tag, seq, call_id).await?;
    Ok(true)
}

pub async fn speed(speed_mode: PlaySpeedModel, _token: String) -> GlobalResult<bool> {
    let (device_id, channel_id, _ssrc) = id_builder::de_stream_id(&*speed_mode.streamId)?;
    let (call_id, seq, from_tag, to_tag) = state::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&speed_mode.streamId)
        .ok_or_else(|| GlobalError::new_biz_error(1100, "流不存在", |msg| error!("{msg}")))?;
    CmdStream::play_speed(&device_id, &channel_id, speed_mode.speedRate, &from_tag, &to_tag, seq, call_id).await?;
    Ok(true)
}

pub async fn ptz(ptz_control_model: PtzControlModel, _token: String) -> GlobalResult<bool> {
    CmdControl::control_ptz(&ptz_control_model).await?;
    let mut model = PtzControlModel::default();
    model.deviceId = ptz_control_model.deviceId.clone();
    sleep(Duration::from_millis(1000)).await;
    model.channelId = ptz_control_model.channelId.clone();
    CmdControl::control_ptz(&model).await?;
    Ok(true)
}

//选择流媒体节点（可用+负载最小）-> 监听流注册
//发起实时点播 -> 监听设备响应
//缓存流信息
async fn start_invite_stream(device_id: &String, channel_id: &String, _token: &String, am: AccessMode, st: u32, et: u32,
                             _trans_mode: Option<TransMode>, custom_media_config: Option<CustomMediaConfig>) -> GlobalResult<(String, String)> {
    let u16ssrc = state::cache::Cache::ssrc_sn_get().ok_or_else(|| GlobalError::new_biz_error(1100, "ssrc已用完,并发达上限,等待释放", |msg| error!("{msg}")))?;
    let mut node_sets = state::cache::Cache::stream_map_order_node();
    let (ssrc, stream_id) = id_builder::build_ssrc_stream_id(device_id, channel_id, u16ssrc, true).await?;
    let u32ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let conf = StreamConf::get_stream_conf();
    let msc = match custom_media_config {
        None => {
            MediaConfig {
                ssrc: u32ssrc,
                stream_id: stream_id.clone(),
                expires: None,
                codec: None,
                filter: Default::default(),
                output:OutputKind::HttpFlv(HttpFlvOutput{fmt:Flv::default()}),
            }
        }
        Some(cmc) => {
            MediaConfig {
                ssrc: u32ssrc,
                stream_id: stream_id.clone(),
                expires: None,
                codec: cmc.codec,
                output: cmc.output,
                filter: cmc.filter,
            }
        }
    };

    //选择负载最小的节点开始尝试：节点是否可用;
    while let Some((_, node_name)) = node_sets.pop_first() {
        let stream_node = conf.node_map.get(&node_name).unwrap();
        let p = HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port).hand_log(|msg| error!("{msg}"))?;
        //next 将sdp支持从session固定的，转为stream支持的
        if let Ok(res) = p.stream_init(&msc).await.hand_log(|msg| error!("{msg}")) {
            if res.code == 200 {
                let (res, media_ext, from_tag, to_tag) = match am {
                    AccessMode::Live => {
                        CmdStream::play_live_invite(device_id, channel_id, &stream_node.pub_ip.to_string(), stream_node.pub_port, StreamMode::Udp, &ssrc).await?
                    }
                    AccessMode::Back => {
                        CmdStream::play_back_invite(device_id, channel_id, &stream_node.pub_ip.to_string(), stream_node.pub_port, StreamMode::Udp, &ssrc, st, et).await?
                    }
                    AccessMode::Down => {
                        CmdStream::download_invite(device_id, channel_id, &stream_node.pub_ip.to_string(), stream_node.pub_port, StreamMode::Udp, &ssrc, st, et, 1).await?
                    }
                };

                //回调给gmv-stream 使其确认媒体类型
                let map = MediaMap {
                    ssrc: u32ssrc,
                    ext: media_ext,
                };
                p.stream_init_ext(&map).await.hand_log(|msg| error!("{msg}"))?;
                let (call_id, seq) = CmdStream::invite_ack(device_id, &res)?;
                return if let Some(_base_stream_info) = listen_stream_by_stream_id(&stream_id, RELOAD_EXPIRES).await {
                    state::cache::Cache::stream_map_insert_info(stream_id.clone(), node_name.clone(), call_id, seq, am, from_tag, to_tag);
                    state::cache::Cache::device_map_insert(device_id.to_string(), channel_id.to_string(), ssrc, stream_id.clone(), am, msc);
                    Ok((stream_id, node_name))
                } else {
                    CmdStream::play_bye(seq + 1, call_id, device_id, channel_id, &from_tag, &to_tag).await?;
                    Err(GlobalError::new_biz_error(1100, "未接收到监控推流", |msg| error!("{msg}")))
                };
            }
        }
    }
    Err(GlobalError::new_biz_error(1100, "无可用流媒体服务", |msg| error!("{msg}")))
}


//首先查看session缓存中是否有映射关系,然后看stream中是否有相应数据:都为true时返回数据
//当session有,stream无时：session调用stream->使其重新监听ssrc
//(避免stream重启后,数据不一致)
// 主动探测ssrc是否存在：
// session不存在：None，
// session存在: 返回Some,

async fn enable_invite_stream(device_id: &String, channel_id: &String, am: &AccessMode) -> GlobalResult<Option<(String, String)>> {
    match state::cache::Cache::device_map_get_invite_info(device_id, channel_id, am) {
        None => {
            Ok(None)
        }
        //session -> true
        Some((stream_id, ssrc)) => {
            let mut res = None;
            if let Some(node_name) = state::cache::Cache::stream_map_query_node_name(&stream_id) {
                //确认stream是否存在
                if let Some(stream_node) = StreamConf::get_stream_conf().node_map.get(&node_name) {
                    let pretend = HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port)?;
                    let ssrc_num = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
                    let stream_key = StreamKey { ssrc: ssrc_num, stream_id: Some(stream_id.clone()) };
                    let json_obj = pretend.stream_online(&stream_key).await.hand_log(|msg| error!("{msg}"))?;
                    if let Some(true) = json_obj.data {
                        //stream -> true
                        res = Some((stream_id.clone(), node_name));
                    }
                }
            }
            //stream中无stream_id映射,同步剔除session中映射
            //向设备发送关闭流
            if res.is_none() {
                state::cache::Cache::device_map_remove(device_id, None);
                state::cache::Cache::stream_map_remove(&stream_id, None);
                let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
                let ssrc_num = (ssrc % 10000) as u16;
                state::cache::Cache::ssrc_sn_set(ssrc_num);
                let cst_info = state::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
                if let Ok((device_id, channel_id, _ssrc_str)) = id_builder::de_stream_id(&stream_id) {
                    if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
                        let _ = CmdStream::play_bye(seq, call_id, &device_id, &channel_id, &from_tag, &to_tag).await;
                    }
                }
            }
            Ok(res)
        }
    }
}


async fn listen_stream_by_stream_id(stream_id: &String, secs: u64) -> Option<BaseStreamInfo> {
    let (tx, mut rx) = mpsc::channel(8);
    let when = Instant::now() + Duration::from_secs(secs);
    let key = format!("{}{stream_id}", KEY_STREAM_IN);
    state::cache::Cache::state_insert(key.clone(), Bytes::new(), Some(when), Some(tx));
    let mut res = None;
    if let Some(Some(bytes)) = rx.recv().await {
        res = serde_json::from_slice::<BaseStreamInfo>(&*bytes).ok();
    }
    state::cache::Cache::state_remove(&key);
    res
}

#[cfg(test)]
mod test {
    use std::time::Duration;
    use base::tokio;
    use base::chrono::Local;
    use base::tokio::sync::mpsc;
    use base::tokio::time::{Instant, sleep_until};

    #[tokio::test]
    async fn test() {
        let (tx, mut rx) = mpsc::channel::<Option<u32>>(8);
        let init = Local::now().timestamp_millis();
        println!("{} : {}", "first init", init);
        tokio::spawn(async move {
            sleep_until(Instant::now() + Duration::from_secs(2)).await;
            tx.send(None).await.unwrap();
            let current = Local::now().timestamp_millis();
            println!("{} : {}", "sub", current - init);
        });
        if let Some(Some(data)) = rx.recv().await {
            println!("res = {}", data);
        }
        let current = Local::now().timestamp_millis();
        println!("{} : {}", "main", current - init);
        sleep_until(Instant::now() + Duration::from_secs(6)).await;
    }

    #[test]
    fn test_u16_from_str() {
        let ssrc = u32::from_str_radix("1100000001", 10);
        println!("{:?}", ssrc);
        assert_eq!(ssrc, Ok(1100000001))
    }
}