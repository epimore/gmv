use std::fs;
use std::path::Path;
use std::time::Duration;

use base::chrono::{Local, TimeZone};
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::tokio::sync::mpsc;
use base::tokio::time::{sleep, Instant};
use shared::info::format::{CMaf, Mp4};
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{BaseStreamInfo, StreamInfoQo, StreamKey, StreamRecordInfo};
use shared::info::output::{DashFmp4Output, LocalMp4Output, OutputEnum, OutputKind};

use crate::gb::handler::cmd::{CmdControl, CmdStream};
use crate::gb::RWContext;
use crate::http::client::{HttpClient, HttpStream};
use crate::service::{EXPIRES, KEY_STREAM_IN};
use crate::state;
use crate::state::model::{
    CustomMediaConfig, PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel,
    PtzControlModel, StreamInfo, StreamQo, TransMode,
};
use crate::state::session::AccessMode;
use crate::state::{session, DownloadConf, StreamConf};
use crate::storage::entity::GmvRecord;
use crate::utils::id_builder;

pub async fn play_live(play_live_model: PlayLiveModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = &play_live_model.device_id;
    if !RWContext::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| {
            error!("{msg}")
        }));
    }
    let channel_id = play_live_model.channel_id.as_ref().unwrap_or(device_id);
    let am = AccessMode::Live;
    let output = play_live_model
        .custom_media_config
        .as_ref()
        .map(|p| p.output.clone());
    let setup_lock = state::session::Cache::stream_setup_lock(device_id, channel_id, am);
    let _setup_guard = setup_lock.lock().await;

    if let Some((stream_id, proxy_addr)) = enable_invite_stream(device_id, channel_id, &am).await?
    {
        state::session::Cache::stream_map_insert_token(stream_id.clone(), token);
        return StreamInfo::build(stream_id, proxy_addr, output);
    }

    let (stream_id, _node_name, proxy_addr) = start_invite_stream(
        device_id,
        channel_id,
        &token,
        am,
        0,
        0,
        play_live_model.trans_mode,
        play_live_model.custom_media_config,
    )
    .await?;
    state::session::Cache::stream_map_insert_token(stream_id.clone(), token);
    StreamInfo::build(stream_id, proxy_addr, output)
}

pub async fn download_info_by_stream_id(
    info: StreamQo,
    _token: String,
) -> GlobalResult<StreamRecordInfo> {
    let (stream_server, ssrc) = session::Cache::stream_map_query_node_ssrc(&info.stream_id)
        .ok_or_else(|| {
            GlobalError::new_biz_error(1100, "无效的媒体流ID", |msg| error!("{msg}"))
        })?;
    let conf = StreamConf::get_stream_conf();
    match conf.node_map.get(&stream_server) {
        None => Err(GlobalError::new_biz_error(
            1100,
            "stream_server 错误",
            |msg| error!("{msg}"),
        )),
        Some(node) => {
            let p = HttpClient::template_ip_port(&node.local_ip.to_string(), node.local_port)?;
            let output_enum = info.media_type.unwrap_or(OutputEnum::LocalMp4);
            let json_obj = p
                .record_info(&StreamInfoQo { ssrc, output_enum })
                .await
                .hand_log(|msg| error!("{msg}"))?;
            let value = json_obj.value();
            if value.code == 200 {
                match value.data {
                    None => Err(GlobalError::new_biz_error(
                        1100,
                        "stream_server 错误",
                        |msg| error!("{msg}: {}", &value.msg),
                    )),
                    Some(info) => Ok(info),
                }
            } else {
                Err(GlobalError::new_biz_error(
                    1100,
                    "stream_server 错误",
                    |msg| error!("{msg}: {}", &value.msg),
                ))
            }
        }
    }
}

pub async fn download_stop(stream_id: String, _token: String) -> GlobalResult<bool> {
    let cst_info = state::session::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
    if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
        let (stream_server, ssrc) = session::Cache::stream_map_query_node_ssrc(&stream_id)
            .ok_or_else(|| {
                GlobalError::new_biz_error(1100, "无效的媒体流ID", |msg| error!("{msg}"))
            })?;
        let conf = StreamConf::get_stream_conf();
        match conf.node_map.get(&stream_server) {
            None => {
                warn!(
                    "close from stream be failed: {};stream server not found",
                    &stream_id
                );
            }
            Some(node) => {
                let p = HttpClient::template_ip_port(&node.local_ip.to_string(), node.local_port)?;
                let json_obj = p
                    .close_output(&StreamInfoQo {
                        ssrc,
                        output_enum: OutputEnum::LocalMp4,
                    })
                    .await
                    .hand_log(|msg| error!("{msg}"))?;
                let value = json_obj.value();
                if value.code != 200 {
                    warn!("close from stream be failed: {}", &stream_id);
                }
            }
        }
        if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
            let _ = CmdStream::play_bye(seq, call_id, &device_id, &channel_id, &from_tag, &to_tag)
                .await;
        }
        if let Some(am) = state::session::Cache::stream_map_query_play_type_by_stream_id(&stream_id)
        {
            state::session::Cache::device_map_remove(
                &device_id,
                Some((&channel_id, Some((am, &ssrc_str)))),
            );
            state::session::Cache::stream_map_remove(&stream_id, None);
        }
        let ssrc = u32::from_str_radix(&ssrc_str, 10).hand_log(|msg| error!("{msg}"))?;
        let ssrc_num = (ssrc % 10000) as u16;
        state::session::Cache::ssrc_sn_set(ssrc_num);
        return Ok(true);
    }
    Ok(false)
}

pub async fn download(play_back_model: PlayBackModel, token: String) -> GlobalResult<String> {
    let device_id = &play_back_model.device_id;
    if !RWContext::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| {
            error!("{msg}")
        }));
    }
    let channel_id = play_back_model.channel_id.as_ref().unwrap_or(device_id);
    let am = AccessMode::Down;
    let setup_lock = state::session::Cache::stream_setup_lock(device_id, channel_id, am);
    let _setup_guard = setup_lock.lock().await;

    if let Some(_) =
        GmvRecord::query_gmv_record_run_by_device_id_channel_id(device_id, channel_id).await?
    {
        return Err(GlobalError::new_biz_error(1000, "任务已存在", |msg| {
            error!("{msg}")
        }));
    }
    let st = play_back_model.st;
    let et = play_back_model.et;

    let storage_path = DownloadConf::get_download_conf().storage_path;
    let date_str = Local::now().format("%Y%m%d").to_string();
    let path = Path::new(&storage_path).join(date_str);
    fs::create_dir_all(&path).hand_log(|msg| error!("{msg}"))?;
    let abs_path = path
        .canonicalize()
        .hand_log(|msg| error!("{msg}"))?
        .to_str()
        .ok_or_else(|| GlobalError::new_sys_error("文件名错误", |msg| error!("{msg}")))?
        .to_string();

    let down_conf = play_back_model
        .custom_media_config
        .clone()
        .unwrap_or_else(|| CustomMediaConfig {
            output: OutputKind::LocalMp4(LocalMp4Output {
                fmt: Mp4::default(),
                path: abs_path.clone(),
            }),
            codec: None,
            filter: Default::default(),
        });
    let (stream_id, node_name, _proxy_addr) = start_invite_stream(
        device_id,
        channel_id,
        &token,
        am,
        st - 2,
        et + 1,
        play_back_model.trans_mode,
        Some(down_conf),
    )
    .await?;
    state::session::Cache::stream_map_insert_token(stream_id.clone(), token);
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
    if !RWContext::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(1000, "设备已离线", |msg| {
            error!("{msg}")
        }));
    }
    let channel_id = play_back_model.channel_id.as_ref().unwrap_or(device_id);
    let am = AccessMode::Back;
    let output = play_back_model
        .custom_media_config
        .as_ref()
        .map(|p| p.output.clone());
    let setup_lock = state::session::Cache::stream_setup_lock(device_id, channel_id, am);
    let _setup_guard = setup_lock.lock().await;

    if let Some((stream_id, proxy_addr)) = enable_invite_stream(device_id, channel_id, &am).await?
    {
        state::session::Cache::stream_map_insert_token(stream_id.clone(), token);
        return StreamInfo::build(stream_id, proxy_addr, output);
    }
    let st = play_back_model.st;
    let et = play_back_model.et;
    let (stream_id, _node_name, proxy_addr) = start_invite_stream(
        device_id,
        channel_id,
        &token,
        am,
        st,
        et,
        play_back_model.trans_mode,
        play_back_model.custom_media_config,
    )
    .await?;
    state::session::Cache::stream_map_insert_token(stream_id.clone(), token);
    StreamInfo::build(stream_id, proxy_addr, output)
}

pub async fn seek(seek_mode: PlaySeekModel, _token: String) -> GlobalResult<bool> {
    let (device_id, channel_id, _ssrc) = id_builder::de_stream_id(&seek_mode.streamId)?;
    let (call_id, seq, from_tag, to_tag) =
        state::session::Cache::stream_map_build_call_id_seq_from_to_tag(&seek_mode.streamId)
            .ok_or_else(|| {
                GlobalError::new_biz_error(1100, "流不存在", |msg| error!("{msg}"))
            })?;
    CmdStream::play_seek(
        &device_id,
        &channel_id,
        seek_mode.seekSecond,
        &from_tag,
        &to_tag,
        seq,
        call_id,
    )
    .await?;
    Ok(true)
}

pub async fn speed(speed_mode: PlaySpeedModel, _token: String) -> GlobalResult<bool> {
    let (device_id, channel_id, _ssrc) = id_builder::de_stream_id(&speed_mode.streamId)?;
    let (call_id, seq, from_tag, to_tag) =
        state::session::Cache::stream_map_build_call_id_seq_from_to_tag(&speed_mode.streamId)
            .ok_or_else(|| {
                GlobalError::new_biz_error(1100, "流不存在", |msg| error!("{msg}"))
            })?;
    CmdStream::play_speed(
        &device_id,
        &channel_id,
        speed_mode.speedRate,
        &from_tag,
        &to_tag,
        seq,
        call_id,
    )
    .await?;
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

async fn start_invite_stream(
    device_id: &String,
    channel_id: &String,
    _token: &String,
    am: AccessMode,
    st: u32,
    et: u32,
    _trans_mode: Option<TransMode>,
    custom_media_config: Option<CustomMediaConfig>,
) -> GlobalResult<(String, String, String)> {
    let u16ssrc = state::session::Cache::ssrc_sn_get().ok_or_else(|| {
        GlobalError::new_biz_error(1100, "ssrc已用完,并发达上限,等待释放", |msg| {
            error!("{msg}")
        })
    })?;
    let mut node_sets = state::session::Cache::stream_map_order_node();
    let live = matches!(am, AccessMode::Live);
    let (ssrc, stream_id) =
        match id_builder::build_ssrc_stream_id(device_id, channel_id, u16ssrc, live).await {
            Ok(value) => value,
            Err(err) => {
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };
    let u32ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let conf = StreamConf::get_stream_conf();
    let msc = match custom_media_config {
        None => MediaConfig {
            ssrc: u32ssrc,
            stream_id: stream_id.clone(),
            in_wait_timeout: None,
            codec: None,
            filter: Default::default(),
            output: OutputKind::DashFmp4(DashFmp4Output {
                fmt: CMaf::default(),
            }),
            out_idle_timeout: None,
        },
        Some(cmc) => MediaConfig {
            ssrc: u32ssrc,
            stream_id: stream_id.clone(),
            in_wait_timeout: None,
            codec: cmc.codec,
            output: cmc.output,
            filter: cmc.filter,
            out_idle_timeout: None,
        },
    };

    while let Some((_, node_name)) = node_sets.pop_first() {
        let stream_node = conf.node_map.get(&node_name).unwrap();
        let client =
            HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port)
                .hand_log(|msg| error!("{msg}"))?;

        let Ok(res) = client.stream_init(&msc).await.hand_log(|msg| error!("{msg}")) else {
            continue;
        };
        if res.code != 200 {
            continue;
        }

        let invite_res = match am {
            AccessMode::Live => {
                CmdStream::play_live_invite(
                    device_id,
                    channel_id,
                    &stream_node.pub_ip.to_string(),
                    stream_node.pub_port,
                    TransMode::Udp,
                    &ssrc,
                )
                .await
            }
            AccessMode::Back => {
                CmdStream::play_back_invite(
                    device_id,
                    channel_id,
                    &stream_node.pub_ip.to_string(),
                    stream_node.pub_port,
                    TransMode::Udp,
                    &ssrc,
                    st,
                    et,
                )
                .await
            }
            AccessMode::Down => {
                CmdStream::download_invite(
                    device_id,
                    channel_id,
                    &stream_node.pub_ip.to_string(),
                    stream_node.pub_port,
                    TransMode::Udp,
                    &ssrc,
                    st,
                    et,
                    1,
                )
                .await
            }
        };
        let (res, media_ext, from_tag, to_tag, association) = match invite_res {
            Ok(value) => value,
            Err(err) => {
                cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };

        let map = MediaMap {
            ssrc: u32ssrc,
            ext: media_ext,
        };
        if let Err(err) = client
            .stream_init_ext(&map)
            .await
            .hand_log(|msg| error!("{msg}"))
        {
            cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
            state::session::Cache::ssrc_sn_set(u16ssrc);
            return Err(err);
        }

        let (call_id, seq) = match CmdStream::invite_ack(device_id, &res, association).await {
            Ok(value) => value,
            Err(err) => {
                cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };

        if let Some(base_stream_info) = listen_stream_by_stream_id(&stream_id, EXPIRES).await {
            state::session::Cache::stream_map_insert_info(
                stream_id.clone(),
                u32ssrc,
                base_stream_info.rtp_info.proxy_addr.clone(),
                node_name.clone(),
                call_id,
                seq,
                am,
                from_tag,
                to_tag,
            );
            state::session::Cache::device_map_insert(
                device_id.to_string(),
                channel_id.to_string(),
                ssrc,
                stream_id.clone(),
                am,
                msc,
            );
            return Ok((stream_id, node_name, base_stream_info.rtp_info.proxy_addr));
        }

        let _ = CmdStream::play_bye(
            seq + 1,
            call_id,
            device_id,
            channel_id,
            &from_tag,
            &to_tag,
        )
        .await;
        cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
        state::session::Cache::ssrc_sn_set(u16ssrc);
        return Err(GlobalError::new_biz_error(1100, "未接收到监控推流", |msg| {
            error!("{msg}")
        }));
    }

    state::session::Cache::ssrc_sn_set(u16ssrc);
    Err(GlobalError::new_biz_error(1100, "无可用流媒体服务", |msg| {
        error!("{msg}")
    }))
}

async fn enable_invite_stream(
    device_id: &String,
    channel_id: &String,
    am: &AccessMode,
) -> GlobalResult<Option<(String, String)>> {
    match state::session::Cache::device_map_get_invite_info(device_id, channel_id, am) {
        None => Ok(None),
        Some((stream_id, ssrc)) => {
            let mut res = None;
            if let Some((node_name, proxy_addr)) =
                state::session::Cache::stream_map_query_node(&stream_id)
            {
                if let Some(stream_node) = StreamConf::get_stream_conf().node_map.get(&node_name) {
                    let pretend = HttpClient::template_ip_port(
                        &stream_node.local_ip.to_string(),
                        stream_node.local_port,
                    )?;
                    let ssrc_num = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
                    let stream_key = StreamKey {
                        ssrc: ssrc_num,
                        stream_id: Some(stream_id.clone()),
                    };
                    let json_obj = pretend
                        .stream_online(&stream_key)
                        .await
                        .hand_log(|msg| error!("{msg}"))?;
                    if let Some(true) = json_obj.data {
                        res = Some((stream_id.clone(), proxy_addr));
                    }
                }
            }

            if res.is_none() {
                let cst_info =
                    state::session::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
                state::session::Cache::device_map_remove(
                    device_id,
                    Some((channel_id, Some((*am, &ssrc)))),
                );
                state::session::Cache::stream_map_remove(&stream_id, None);
                let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
                let ssrc_num = (ssrc % 10000) as u16;
                state::session::Cache::ssrc_sn_set(ssrc_num);
                if let Ok((device_id, channel_id, _ssrc_str)) =
                    id_builder::de_stream_id(&stream_id)
                {
                    if let Some((call_id, seq, from_tag, to_tag)) = cst_info {
                        let _ = CmdStream::play_bye(
                            seq,
                            call_id,
                            &device_id,
                            &channel_id,
                            &from_tag,
                            &to_tag,
                        )
                        .await;
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
    state::session::Cache::insert_stream_wait(key.clone(), when, tx);
    let res = match rx.recv().await {
        Some(Some(info)) => Some(info),
        _ => None,
    };
    state::session::Cache::remove_state(&key);
    res
}

fn output_kind_to_enum(output: &OutputKind) -> OutputEnum {
    match output {
        OutputKind::Rtmp(_) => OutputEnum::Rtmp,
        OutputKind::DashFmp4(_) => OutputEnum::DashFmp4,
        OutputKind::DashMp4(_) => OutputEnum::DashMp4,
        OutputKind::HlsFmp4(_) => OutputEnum::HlsFmp4,
        OutputKind::HlsTs(_) => OutputEnum::HlsTs,
        OutputKind::Rtsp(_) => OutputEnum::Rtsp,
        OutputKind::Gb28181Frame(_) => OutputEnum::Gb28181Frame,
        OutputKind::Gb28181Ps(_) => OutputEnum::Gb28181Ps,
        OutputKind::WebRtc(_) => OutputEnum::WebRtc,
        OutputKind::LocalMp4(_) => OutputEnum::LocalMp4,
        OutputKind::LocalTs(_) => OutputEnum::LocalTs,
        OutputKind::HttpFlv(_) => OutputEnum::HttpFlv,
    }
}

async fn cleanup_stream_init(client: &impl HttpStream, ssrc: u32, output: &OutputKind) {
    let _ = client
        .close_output(&StreamInfoQo {
            ssrc,
            output_enum: output_kind_to_enum(output),
        })
        .await;
}
