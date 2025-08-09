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
use shared::info::format::{Flv, Muxer};
use shared::info::output::{HttpFlv, Output};
use shared::info::media_info::{Converter, MediaStreamConfig};
use shared::info::media_info_ext::{MediaExt, MediaMap, MediaType};
use shared::info::obj::{BaseStreamInfo, StreamKey, StreamPlayInfo, StreamRecordInfo, StreamState};
use shared::info::res::Resp;
use crate::gb::handler::cmd::{CmdControl, CmdStream};
use crate::gb::RWSession;
use crate::general;
use crate::general::cache::AccessMode;
use crate::general::{DownloadConf, StreamConf};
use crate::general::model::{CustomMediaConfig, PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel, PtzControlModel, SingleParam, StreamInfo, StreamMode, TransMode};
use crate::service::{RELOAD_EXPIRES};
use crate::storage::entity::{GmvFileInfo, GmvRecord};
use crate::utils::{id_builder};
use crate::http::client::{HttpClient, HttpStream};

const KEY_STREAM_IN: &str = "KEY_STREAM_IN:";

pub fn on_play(stream_play_info: StreamPlayInfo) -> bool {
    let gmv_token = stream_play_info.token;
    let stream_id = stream_play_info.base_stream_info.stream_id;
    general::cache::Cache::stream_map_contains_token(&stream_id, &gmv_token)
}

//无人观看则关闭流
pub async fn stream_idle(base_stream_info: BaseStreamInfo) {
    let stream_id = base_stream_info.stream_id;
    let cst_info = general::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
    if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
        if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
            let _ = CmdStream::play_bye(seq, call_id, &device_id, &channel_id, &from_tag, &to_tag).await;
        }
        if let Some(am) = general::cache::Cache::stream_map_query_play_type_by_stream_id(&stream_id) {
            general::cache::Cache::device_map_remove(&device_id, Some((&channel_id, Some((am, &ssrc_str)))));
            general::cache::Cache::stream_map_remove(&stream_id, None);
        }
        let ssrc = base_stream_info.rtp_info.ssrc;
        let ssrc_num = (ssrc % 10000) as u16;
        general::cache::Cache::ssrc_sn_set(ssrc_num);
    }
}

pub async fn off_play(stream_play_info: StreamPlayInfo) {
    let stream_id = stream_play_info.base_stream_info.stream_id;
    let gmv_token = stream_play_info.token;
    general::cache::Cache::stream_map_remove(&stream_id, Some(&gmv_token));
}

pub async fn end_record(stream_record_info: StreamRecordInfo) {
    if let Some(path_file_name) = stream_record_info.file_name {
        if let Ok((abs_path, dir_path, biz_id, extension)) = get_path(&path_file_name) {
            if let Ok(Some(mut record)) = GmvRecord::query_gmv_record_by_biz_id(&biz_id).await {
                let total_secs = record.et.sub(record.st).num_seconds();
                let per = (stream_record_info.timestamp as i64) * 100 / total_secs;
                if per > 5 { record.state = 2; } else { record.state = 1; }
                record.lt = Local::now().naive_local();
                if let Ok(_) = record.update_gmv_record_by_biz_id().await {
                    let file_info = GmvFileInfo {
                        id: None,
                        device_id: record.device_id,
                        channel_id: record.channel_id,
                        biz_time: Some(Local::now().naive_local()),
                        biz_id,
                        file_type: Some(1),
                        file_size: stream_record_info.file_size,
                        file_name: record.biz_id,
                        file_format: Some(extension),
                        dir_path,
                        abs_path,
                        note: None,
                        is_del: Some(0),
                        create_time: Some(Local::now().naive_local()),
                    };
                    let _ = GmvFileInfo::insert_gmv_file_info(vec![file_info]).await;
                }
            }
        }
    };
}

fn get_path(path_file_name: &String) -> GlobalResult<(String, String, String, String)> {
    let path = Path::new(&path_file_name);
    let biz_id = path.file_stem().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
    let extension = path.extension().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
    let p_path = path.parent().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?;
    let l_path = p_path.file_name().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?;
    let d_path = DownloadConf::get_download_conf().storage_path;
    let dir_path = Path::new(&d_path).join(l_path).to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
    let abs_path = p_path.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();
    Ok((abs_path, dir_path, biz_id, extension))
}

pub async fn stream_register(base_stream_info: BaseStreamInfo) {
    let key_stream_in_id = format!("{KEY_STREAM_IN}{}", base_stream_info.stream_id);
    if let Some((_, Some(tx))) = general::cache::Cache::state_get(&key_stream_in_id) {
        let vec = serde_json::to_vec(&base_stream_info).unwrap();
        let bytes = Bytes::from(vec);
        let _ = tx.try_send(Some(bytes)).hand_log(|msg| error!("{msg}"));
    }
}

//gmv-stream接收流超时:还ssrc_sn,清理stream_map/device_map
pub fn stream_input_timeout(stream_state: StreamState) {
    let ssrc = stream_state.base_stream_info.rtp_info.ssrc;
    let ssrc_num = (ssrc % 10000) as u16;
    general::cache::Cache::ssrc_sn_set(ssrc_num);
    let stream_id = stream_state.base_stream_info.stream_id;
    if let Some(am) = general::cache::Cache::stream_map_query_play_type_by_stream_id(&stream_id) {
        general::cache::Cache::stream_map_remove(&stream_id, None);
        if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
            general::cache::Cache::device_map_remove(&device_id, Some((&channel_id, Some((am, &ssrc_str)))));
        }
    }
}

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
        general::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
        return Ok(StreamInfo::build(stream_id, node_name));
    }
    let (stream_id, node_name) = start_invite_stream(device_id, channel_id, &token, am, 0, 0, play_live_model.trans_mode, play_live_model.custom_media_config).await?;
    general::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
    Ok(StreamInfo::build(stream_id, node_name))
}

pub async fn download_info_by_stream_id(stream_id: String, stream_server: String, token: String) -> GlobalResult<StreamRecordInfo> {
    let conf = StreamConf::get_stream_conf();
    match conf.node_map.get(&stream_server) {
        None => { Err(GlobalError::new_biz_error(1100, "stream_server 错误", |msg| error!("{msg}"))) }
        Some(node) => {
            let p = HttpClient::template_ip_port(&node.local_ip.to_string(), node.local_port)?;
            let pretend::Json { value } = p.record_info(&SingleParam { param: stream_id }).await.hand_log(|msg| error!("{msg}"))?;
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
    let cst_info = general::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
    if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
        if let Ok(Some(mut record)) = GmvRecord::query_gmv_record_by_biz_id(&stream_id).await {
            record.state = 3;
            record.lt = Local::now().naive_local();
            record.update_gmv_record_by_biz_id().await?;
        }
        if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
            let _ = CmdStream::play_bye(seq, call_id, &device_id, &channel_id, &from_tag, &to_tag).await;
        }
        if let Some(am) = general::cache::Cache::stream_map_query_play_type_by_stream_id(&stream_id) {
            general::cache::Cache::device_map_remove(&device_id, Some((&channel_id, Some((am, &ssrc_str)))));
            general::cache::Cache::stream_map_remove(&stream_id, None);
        }
        let ssrc = u32::from_str_radix(&ssrc_str, 10).hand_log(|msg| error!("{msg}"))?;
        let ssrc_num = (ssrc % 10000) as u16;
        general::cache::Cache::ssrc_sn_set(ssrc_num);
        return Ok(true);
    }
    Ok(false)
}

pub async fn download(play_back_model: PlayBackModel, token: String) -> GlobalResult<String> {
    unimplemented!();
    /*let device_id = play_back_model.get_device_id();
    if !RWSession::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| error!("{msg}")));
    }
    let channel_id = if let Some(channel_id) = play_back_model.get_channel_id() {
        channel_id
    } else {
        device_id
    };
    let am = PlayType::Down;
    //查看是否有任务
    if let Some(_) = GmvRecord::query_gmv_record_run_by_device_id_channel_id(device_id, channel_id).await? {
        return Err(GlobalError::new_biz_error(1000, "任务已存在", |msg| error!("{msg}")));
    }
    let st = play_back_model.get_st();
    let et = play_back_model.get_et();

    let storage_path = DownloadConf::get_download_conf().storage_path;
    let date_str = Local::now().format("%Y%m%d").to_string();
    let path = Path::new(&storage_path).join(date_str);
    fs::create_dir_all(&path).hand_log(|msg| error!("{msg}"))?;
    let abs_path = path.canonicalize().hand_log(|msg| error!("{msg}"))?.to_str().ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?.to_string();

    let (stream_id, node_name) = start_invite_stream(device_id, channel_id, &token, am, *st - 2, *et + 1, MediaAction::Download(Download::Mp4(abs_path, None))).await?;
    general::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
    let record = GmvRecord {
        biz_id: stream_id.clone(),
        device_id: device_id.to_string(),
        channel_id: channel_id.to_string(),
        user_id: None,
        st: Local.timestamp_opt(*st as i64, 0).unwrap().naive_local(),
        et: Local.timestamp_opt(*et as i64, 0).unwrap().naive_local(),
        speed: 1,
        ct: Local::now().naive_local(),
        state: 0,
        lt: Local::now().naive_local(),
        stream_app_name: node_name,
    };
    record.insert_single_gmv_record().await?;
    Ok(stream_id)*/
}

pub async fn play_back(play_back_model: PlayBackModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = play_back_model.get_device_id();
    if !RWSession::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| error!("{msg}")));
    }
    let channel_id = if let Some(channel_id) = play_back_model.get_channel_id() {
        channel_id
    } else {
        device_id
    };
    let am = AccessMode::Back;
    //查看流是否已存在,有则直接返回
    if let Some((stream_id, node_name)) = enable_invite_stream(device_id, channel_id, &token, &am, MediaAction::Play(Play::Flv)).await? {
        general::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
        return Ok(StreamInfo::build(stream_id, node_name));
    }
    let st = play_back_model.get_st();
    let et = play_back_model.get_et();
    let (stream_id, node_name) = start_invite_stream(device_id, channel_id, &token, am, *st, *et, MediaAction::Play(Play::Flv)).await?;
    general::cache::Cache::stream_map_insert_token(stream_id.clone(), token);
    Ok(StreamInfo::build(stream_id, node_name))
}

pub async fn seek(seek_mode: PlaySeekModel, _token: String) -> GlobalResult<bool> {
    let (device_id, channel_id, _ssrc) = id_builder::de_stream_id(seek_mode.get_streamId())?;
    let (call_id, seq, from_tag, to_tag) = general::cache::Cache::stream_map_build_call_id_seq_from_to_tag(seek_mode.get_streamId())
        .ok_or_else(|| GlobalError::new_biz_error(1100, "流不存在", |msg| error!("{msg}")))?;
    CmdStream::play_seek(&device_id, &channel_id, *seek_mode.get_seekSecond(), &from_tag, &to_tag, seq, call_id).await?;
    Ok(true)
}

pub async fn speed(speed_mode: PlaySpeedModel, _token: String) -> GlobalResult<bool> {
    let (device_id, channel_id, _ssrc) = id_builder::de_stream_id(speed_mode.get_streamId())?;
    let (call_id, seq, from_tag, to_tag) = general::cache::Cache::stream_map_build_call_id_seq_from_to_tag(speed_mode.get_streamId())
        .ok_or_else(|| GlobalError::new_biz_error(1100, "流不存在", |msg| error!("{msg}")))?;
    CmdStream::play_speed(&device_id, &channel_id, *speed_mode.get_speedRate(), &from_tag, &to_tag, seq, call_id).await?;
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
async fn start_invite_stream(device_id: &String, channel_id: &String, token: &String, am: AccessMode, st: u32, et: u32,
                             trans_mode: Option<TransMode>, custom_media_config: Option<CustomMediaConfig>) -> GlobalResult<(String, String)> {
    let u16ssrc = general::cache::Cache::ssrc_sn_get().ok_or_else(|| GlobalError::new_biz_error(1100, "ssrc已用完,并发达上限,等待释放", |msg| error!("{msg}")))?;
    let mut node_sets = general::cache::Cache::stream_map_order_node();
    let (ssrc, stream_id) = id_builder::build_ssrc_stream_id(device_id, channel_id, u16ssrc, true).await?;
    let u32ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let conf = StreamConf::get_stream_conf();
    let msc = match custom_media_config {
        None => {
            let output = Output {
                http_flv: Some(HttpFlv {}),
                ..Default::default()
            };
            MediaStreamConfig {
                ssrc: u32ssrc,
                stream_id: stream_id.clone(),
                expires: None,
                converter: Converter::default(),
                output,
            }
        }
        Some(cmc) => {
            MediaStreamConfig {
                ssrc: u32ssrc,
                stream_id: stream_id.clone(),
                expires: None,
                converter: cmc.converter,
                output: cmc.output,
            }
        }
    };

    //选择负载最小的节点开始尝试：节点是否可用;
    while let Some((_, node_name)) = node_sets.pop_first() {
        let stream_node = conf.node_map.get(&node_name).unwrap();
        let p = HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port).hand_log(|msg| error!("{msg}"))?;

        //next 将sdp支持从session固定的，转为stream支持的
        if let Json { value: Resp { code: 200, .. } } = p.stream_init(&msc).await.hand_log(|msg| error!("{msg}"))?
        {
            let (res, media_map, from_tag, to_tag) = match am {
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
            MediaMap {
                ssrc: u32ssrc,
                ext: MediaExt {
                    mt: MediaType::Video,
                    tp_code: 0,
                    tp_val: "".to_string(),
                    link_ssrc: None,
                },
            }
            p.stream_init_ext()


            let _ = callback::ident_rtp_media_info(&ssrc, media_map, token, stream_node.get_local_ip(), stream_node.get_local_port()).await;
            let (call_id, seq) = CmdStream::invite_ack(device_id, &res)?;
            return if let Some(_base_stream_info) = listen_stream_by_stream_id(&stream_id, RELOAD_EXPIRES).await {
                general::cache::Cache::stream_map_insert_info(stream_id.clone(), node_name.clone(), call_id, seq, am, from_tag, to_tag);
                general::cache::Cache::device_map_insert(device_id.to_string(), channel_id.to_string(), ssrc, stream_id.clone(), am);
                Ok((stream_id, node_name))
            } else {
                CmdStream::play_bye(seq + 1, call_id, device_id, channel_id, &from_tag, &to_tag).await?;
                Err(GlobalError::new_biz_error(1100, "未接收到监控推流", |msg| error!("{msg}")))
            };
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
    match general::cache::Cache::device_map_get_invite_info(device_id, channel_id, am) {
        None => {
            Ok(None)
        }
        //session -> true
        Some((stream_id, ssrc)) => {
            let mut res = None;
            if let Some(node_name) = general::cache::Cache::stream_map_query_node_name(&stream_id) {
                //确认stream是否存在
                if let Some(stream_node) = StreamConf::get_stream_conf().node_map.get(&node_name) {
                    let pretend = HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port)?;
                    let ssrc_num = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
                    let stream_key = StreamKey { ssrc: ssrc_num, stream_id: Some(stream_id.clone()) };
                    let pretend::Json { value } = pretend.stream_online(&stream_key).await.hand_log(|msg| error!("{msg}"))?;
                    if let Some(true) = value.data {
                        //stream -> true
                        res = Some((stream_id.clone(), node_name));
                    }
                }
            }
            //stream中无stream_id映射,同步剔除session中映射
            //向设备发送关闭流
            if res.is_none() {
                general::cache::Cache::device_map_remove(device_id, None);
                general::cache::Cache::stream_map_remove(&stream_id, None);
                let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
                let ssrc_num = (ssrc % 10000) as u16;
                general::cache::Cache::ssrc_sn_set(ssrc_num);
                let cst_info = general::cache::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
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
    let key = format!("{KEY_STREAM_IN}{stream_id}");
    general::cache::Cache::state_insert(key.clone(), Bytes::new(), Some(when), Some(tx));
    let mut res = None;
    if let Some(Some(bytes)) = rx.recv().await {
        res = serde_json::from_slice::<BaseStreamInfo>(&*bytes).ok();
    }
    general::cache::Cache::state_remove(&key);
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