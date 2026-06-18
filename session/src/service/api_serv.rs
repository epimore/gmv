use std::fs;
use std::path::Path;
use std::time::Duration;

use base::chrono::{Local, TimeZone};
use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use base::tokio::sync::mpsc;
use base::tokio::time::{Instant, sleep};
use shared::info::format::{CMaf, Mp4};
use shared::info::media_info::MediaConfig;
use shared::info::media_info_ext::MediaMap;
use shared::info::obj::{BaseStreamInfo, StreamInfoQo, StreamKey, StreamRecordInfo};
use shared::info::obj::{TalkAnswerReq, TalkInfo, TalkOpenReq, TalkStartModel, TalkStopModel};
use shared::info::output::{DashFmp4Output, LocalMp4Output, OutputEnum, OutputKind};
use shared::info::res::Resp;

use crate::gb::SessionConf;
use crate::gb::sip::InviteTalkRequest;
use crate::gb::sip::command as sip_command;
use crate::http::client::{HttpClient, HttpStream};
use crate::register::core::Register;
use crate::service::talk::{
    DEFAULT_TALK_INPUT_TIMEOUT_SECS, TalkAudioOptions, append_gmv_token, cleanup_talk_open,
    parse_talk_answer, sip_command_target, stream_resp_data, talk_codec_to_pjsip,
};
use crate::service::{EXPIRES, KEY_STREAM_IN, stream_close, talk_close};
use crate::state;
use crate::state::model::{
    CustomMediaConfig, PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel,
    PtzControlModel, StreamInfo, StreamQo, TransMode,
};
use crate::state::session::AccessMode;
use crate::state::session::TalkSessionState;
use crate::state::{DownloadConf, StreamConf, session};
use crate::storage::dialog_session::{DialogState, SipDialogSessionRepository};
use crate::storage::entity::GmvRecord;
use crate::utils::id_builder;
use gmv_pjsip::TalkSdpMode;

pub async fn play_live(play_live_model: PlayLiveModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = &play_live_model.device_id;
    if !Register::has_session(device_id) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Network.code(),
            "设备已离线",
            |msg| error!("{msg}"),
        ));
    }
    let channel_id = play_live_model.channel_id.as_ref().unwrap_or(device_id);
    let am = AccessMode::Live;
    let output = play_live_model
        .custom_media_config
        .as_ref()
        .map(|p| p.output.clone());
    let setup_lock = state::session::Cache::stream_setup_lock(device_id, channel_id, am);
    let _setup_guard = setup_lock.lock().await;

    if let Some((stream_id, proxy_addr)) = enable_invite_stream(device_id, channel_id, &am).await? {
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
            GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "无效的媒体流ID",
                |msg| error!("{msg}"),
            )
        })?;
    let conf = StreamConf::get_stream_conf();
    match conf.node_map.get(&stream_server) {
        None => Err(GlobalError::new_biz_error(
            BaseErrorCode::NotFound.code(),
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
                        BaseErrorCode::NotFound.code(),
                        "stream_server 错误",
                        |msg| error!("{msg}: {}", &value.msg),
                    )),
                    Some(info) => Ok(info),
                }
            } else {
                Err(GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "stream_server 错误",
                    |msg| error!("{msg}: {}", &value.msg),
                ))
            }
        }
    }
}

pub async fn download_stop(stream_id: String, _token: String) -> GlobalResult<bool> {
    if id_builder::de_stream_id(&stream_id).is_ok() {
        let (stream_server, ssrc) = session::Cache::stream_map_query_node_ssrc(&stream_id)
            .ok_or_else(|| {
                GlobalError::new_biz_error(
                    BaseErrorCode::InvalidRequest.code(),
                    "无效的媒体流ID",
                    |msg| error!("{msg}"),
                )
            })?;
        stream_close::begin(stream_id.clone());
        let conf = StreamConf::get_stream_conf();
        match conf.node_map.get(&stream_server) {
            None => {
                return Err(GlobalError::new_biz_error(
                    BaseErrorCode::NotFound.code(),
                    "stream server not found",
                    |msg| error!("{msg}: stream_id={stream_id}, node={stream_server}"),
                ));
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
                    return Err(GlobalError::new_biz_error(value.code, &value.msg, |msg| {
                        error!("close stream output failed: {msg}, stream_id={stream_id}")
                    }));
                }
            }
        }
        return Ok(true);
    }
    Ok(false)
}

pub async fn download(play_back_model: PlayBackModel, token: String) -> GlobalResult<String> {
    let device_id = &play_back_model.device_id;
    if !Register::has_session(device_id) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Network.code(),
            "设备已离线",
            |msg| error!("{msg}"),
        ));
    }
    let channel_id = play_back_model.channel_id.as_ref().unwrap_or(device_id);
    let am = AccessMode::Down;
    let setup_lock = state::session::Cache::stream_setup_lock(device_id, channel_id, am);
    let _setup_guard = setup_lock.lock().await;

    if let Some(_) =
        GmvRecord::query_gmv_record_run_by_device_id_channel_id(device_id, channel_id).await?
    {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::AlreadyExists.code(),
            "任务已存在",
            |msg| error!("{msg}"),
        ));
    }
    let st = play_back_model.st;
    let et = play_back_model.et;
    validate_playback_range(st, et)?;

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
        st.saturating_sub(2),
        et.saturating_add(1),
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
        st: Local
            .timestamp_opt(st as i64, 0)
            .single()
            .ok_or_else(|| GlobalError::new_sys_error("invalid start time", |msg| error!("{msg}")))?
            .naive_local(),
        et: Local
            .timestamp_opt(et as i64, 0)
            .single()
            .ok_or_else(|| GlobalError::new_sys_error("invalid end time", |msg| error!("{msg}")))?
            .naive_local(),
        speed: 1,
        ct: Local::now().naive_local(),
        state: 0,
        lt: Local::now().naive_local(),
        stream_app_name: node_name,
    };
    if let Err(err) = record.insert_single_gmv_record().await {
        stream_close::begin(stream_id.clone());
        return Err(err);
    }
    Ok(stream_id)
}

pub async fn play_back(play_back_model: PlayBackModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = &play_back_model.device_id;
    if !Register::has_session(device_id) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Network.code(),
            "设备已离线",
            |msg| error!("{msg}"),
        ));
    }
    let channel_id = play_back_model.channel_id.as_ref().unwrap_or(device_id);
    let st = play_back_model.st;
    let et = play_back_model.et;
    validate_playback_range(st, et)?;
    let am = AccessMode::Back;
    let output = play_back_model
        .custom_media_config
        .as_ref()
        .map(|p| p.output.clone());
    let setup_lock = state::session::Cache::stream_setup_lock(device_id, channel_id, am);
    let _setup_guard = setup_lock.lock().await;

    if let Some((stream_id, proxy_addr)) = enable_invite_stream(device_id, channel_id, &am).await? {
        state::session::Cache::stream_map_insert_token(stream_id.clone(), token);
        return StreamInfo::build(stream_id, proxy_addr, output);
    }
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
    let device_id = playback_stream_device(&seek_mode.streamId)?;
    sip_command::play_seek(&device_id, &seek_mode.streamId, seek_mode.seekSecond).await?;
    Ok(true)
}

pub async fn speed(speed_mode: PlaySpeedModel, _token: String) -> GlobalResult<bool> {
    if !matches!(speed_mode.speedRate, 0.5 | 1.0 | 2.0 | 4.0) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "unsupported playback speed",
            |msg| error!("{msg}: speed={}", speed_mode.speedRate),
        ));
    }
    let device_id = playback_stream_device(&speed_mode.streamId)?;
    sip_command::play_speed(&device_id, &speed_mode.streamId, speed_mode.speedRate).await?;
    Ok(true)
}

pub async fn ptz(ptz_control_model: PtzControlModel, _token: String) -> GlobalResult<bool> {
    sip_command::control_ptz(&ptz_control_model).await?;
    let mut model = PtzControlModel::default();
    model.deviceId = ptz_control_model.deviceId.clone();
    sleep(Duration::from_millis(1000)).await;
    model.channelId = ptz_control_model.channelId.clone();
    sip_command::control_ptz(&model).await?;
    Ok(true)
}

pub async fn talk_start(model: TalkStartModel, token: String) -> GlobalResult<TalkInfo> {
    let device_id = &model.device_id;
    if !Register::has_session(device_id) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Network.code(),
            "设备已离线",
            |msg| error!("{msg}"),
        ));
    }

    let channel_id = model
        .channel_id
        .clone()
        .unwrap_or_else(|| device_id.clone());
    let audio = TalkAudioOptions::try_from_model(&model)?;
    let setup_lock =
        state::session::Cache::stream_setup_lock(device_id, &channel_id, AccessMode::Talk);
    let _setup_guard = setup_lock.lock().await;

    if state::session::Cache::talk_map_get_by_device_channel(device_id, &channel_id).is_some() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::AlreadyExists.code(),
            "talk session already exists",
            |msg| error!("{msg}: device_id={device_id}, channel_id={channel_id}"),
        ));
    }

    let u16ssrc = state::session::Cache::ssrc_sn_get().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::IoBusy.code(),
            "ssrc已用完,并发达上限,等待释放",
            |msg| error!("{msg}"),
        )
    })?;
    let (ssrc, talk_id) =
        match id_builder::build_ssrc_stream_id(device_id, &channel_id, u16ssrc, true).await {
            Ok(value) => value,
            Err(err) => {
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };
    let u32ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
    let conf = StreamConf::get_stream_conf();
    let mut node_sets = state::session::Cache::stream_map_order_node();

    while let Some((_, node_name)) = node_sets.pop_first() {
        let Some(stream_node) = conf.node_map.get(&node_name) else {
            continue;
        };
        let client =
            HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port)
                .hand_log(|msg| error!("{msg}"))?;
        let open_req = TalkOpenReq {
            talk_id: talk_id.clone(),
            ssrc: u32ssrc,
            token: token.clone(),
            codec: audio.codec.clone(),
            sample_rate: audio.sample_rate,
            channel_count: audio.channel_count,
            payload_type: audio.payload_type,
            frame_duration_ms: audio.frame_duration_ms,
            input_timeout_secs: DEFAULT_TALK_INPUT_TIMEOUT_SECS,
        };
        let open_resp = match client
            .talk_open(&open_req)
            .await
            .hand_log(|msg| error!("{msg}"))
        {
            Ok(resp) => match stream_resp_data(resp.value(), "talk_open") {
                Ok(data) => data,
                Err(err) => {
                    warn!("talk_open rejected by stream node {}: {:?}", node_name, err);
                    continue;
                }
            },
            Err(err) => {
                warn!("talk_open failed on stream node {}: {:?}", node_name, err);
                continue;
            }
        };

        let (host, port, proto) = match sip_command_target(device_id) {
            Ok(value) => value,
            Err(err) => {
                cleanup_talk_open(client.as_ref(), &talk_id).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };
        let protocol = sip_command::transport_protocol(audio.trans_mode, proto);
        let accepted = match sip_command::talk_invite_and_wait(InviteTalkRequest {
            device_id: device_id.clone(),
            channel_id: channel_id.clone(),
            talk_id: talk_id.clone(),
            device_host: host,
            device_port: port,
            media_ip: stream_node.pub_ip.to_string(),
            media_port: open_resp.rtp_port,
            ssrc: u32ssrc,
            payload_type: open_resp.payload_type,
            codec: talk_codec_to_pjsip(&open_resp.codec),
            mode: TalkSdpMode::SendRecv,
            protocol,
            call_id: None,
            cseq: None,
            subject: None,
        })
        .await
        {
            Ok(value) => value,
            Err(err) => {
                cleanup_talk_open(client.as_ref(), &talk_id).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };

        let answer = match parse_talk_answer(&accepted) {
            Ok(value) => value,
            Err(err) => {
                let _ = sip_command::invite_stop_by_device(
                    device_id,
                    crate::gb::sip::InviteStopRequest {
                        call_id: Some(accepted.call_id.clone()),
                        stream_id: Some(talk_id.clone()),
                    },
                )
                .await;
                cleanup_talk_open(client.as_ref(), &talk_id).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };

        if !audio.compatible_answer(&answer.codec, answer.sample_rate) {
            let _ = sip_command::invite_stop_by_device(
                device_id,
                crate::gb::sip::InviteStopRequest {
                    call_id: Some(accepted.call_id.clone()),
                    stream_id: Some(talk_id.clone()),
                },
            )
            .await;
            cleanup_talk_open(client.as_ref(), &talk_id).await;
            state::session::Cache::ssrc_sn_set(u16ssrc);
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "device talk audio codec is unsupported",
                |msg| {
                    error!(
                        "{msg}: codec={}, sample_rate={}",
                        answer.codec, answer.sample_rate
                    )
                },
            ));
        }

        let answer_req = TalkAnswerReq {
            talk_id: talk_id.clone(),
            device_ip: answer.device_ip,
            device_port: answer.device_port,
            protocol: answer.protocol.get_value().to_string(),
            payload_type: answer.payload_type,
        };
        if let Err(err) = client
            .talk_answer(&answer_req)
            .await
            .hand_log(|msg| error!("{msg}"))
            .and_then(|resp| stream_resp_unit(resp.value(), "talk_answer"))
        {
            let _ = sip_command::invite_stop_by_device(
                device_id,
                crate::gb::sip::InviteStopRequest {
                    call_id: Some(accepted.call_id.clone()),
                    stream_id: Some(talk_id.clone()),
                },
            )
            .await;
            cleanup_talk_open(client.as_ref(), &talk_id).await;
            state::session::Cache::ssrc_sn_set(u16ssrc);
            return Err(err);
        }

        let talk_state = TalkSessionState {
            talk_id: talk_id.clone(),
            device_id: device_id.clone(),
            channel_id: channel_id.clone(),
            ssrc: u32ssrc,
            stream_node_name: node_name,
            call_id: accepted.call_id.clone(),
            seq: 1,
            closing_generation: None,
            bye_inflight_seq: None,
            close_last_error: None,
        };
        if !state::session::Cache::talk_map_insert(talk_state.clone()) {
            let _ = sip_command::invite_stop_by_device(
                device_id,
                crate::gb::sip::InviteStopRequest {
                    call_id: Some(talk_state.call_id.clone()),
                    stream_id: Some(talk_id.clone()),
                },
            )
            .await;
            cleanup_talk_open(client.as_ref(), &talk_id).await;
            state::session::Cache::ssrc_sn_set(u16ssrc);
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::AlreadyExists.code(),
                "talk session already exists",
                |msg| error!("{msg}: talk_id={talk_id}"),
            ));
        }

        return Ok(TalkInfo {
            talk_id,
            input_url: append_gmv_token(open_resp.input_url, &token),
            codec: open_resp.codec,
            sample_rate: open_resp.sample_rate,
            channel_count: open_resp.channel_count,
            frame_duration_ms: open_resp.frame_duration_ms,
        });
    }

    state::session::Cache::ssrc_sn_set(u16ssrc);
    Err(GlobalError::new_biz_error(
        BaseErrorCode::Network.code(),
        "无可用流媒体服务",
        |msg| error!("{msg}"),
    ))
}

pub async fn talk_stop(model: TalkStopModel, _token: String) -> GlobalResult<bool> {
    let Some(talk) = state::session::Cache::talk_map_get(&model.talk_id) else {
        return Ok(false);
    };
    let started = talk_close::begin(model.talk_id);

    if let Some(stream_node) = StreamConf::get_stream_conf()
        .node_map
        .get(&talk.stream_node_name)
    {
        match HttpClient::template_ip_port(
            &stream_node.local_ip.to_string(),
            stream_node.local_port,
        ) {
            Ok(client) => {
                cleanup_talk_open(client.as_ref(), &talk.talk_id).await;
            }
            Err(err) => {
                warn!(
                    "talk_close client build failed: talk_id={}, err={:?}",
                    talk.talk_id, err
                );
            }
        }
    }

    Ok(started)
}

pub async fn peer_dialog_terminated(call_id: String) -> bool {
    persist_peer_dialog_terminated(&call_id).await;
    if let Some(stream) = state::session::Cache::stream_terminated_by_call_id(&call_id) {
        warn!(
            "stream dialog terminated by device: device_id={}, channel_id={}, stream_id={}, \
             ssrc={}, call_id={}",
            stream.device_id, stream.channel_id, stream.stream_id, stream.ssrc, stream.call_id
        );
        return true;
    }
    let Some(talk) = state::session::Cache::talk_map_remove_by_call_id(&call_id) else {
        return false;
    };
    if let Some(stream_node) = StreamConf::get_stream_conf()
        .node_map
        .get(&talk.stream_node_name)
    {
        match HttpClient::template_ip_port(
            &stream_node.local_ip.to_string(),
            stream_node.local_port,
        ) {
            Ok(client) => cleanup_talk_open(client.as_ref(), &talk.talk_id).await,
            Err(err) => warn!(
                "peer BYE talk cleanup client build failed: talk_id={}, err={:?}",
                talk.talk_id, err
            ),
        }
    }
    state::session::Cache::ssrc_sn_set((talk.ssrc % 10000) as u16);
    true
}

async fn persist_peer_dialog_terminated(call_id: &str) {
    let sessions = match SipDialogSessionRepository::find_by_call_id(call_id).await {
        Ok(sessions) => sessions,
        Err(err) => {
            error!("lookup peer-terminated dialog failed: call_id={call_id}; err={err}");
            return;
        }
    };
    let current_node_id = SessionConf::get_session_by_conf().domain_id;
    for session in sessions {
        if session.signal_node_id != current_node_id
            || !matches!(
                session.state,
                DialogState::Established | DialogState::Terminating
            )
        {
            continue;
        }
        match SipDialogSessionRepository::cas_transition(
            &session.stream_id,
            &session.signal_node_id,
            session.version,
            session.state,
            DialogState::Terminated,
            Local::now().naive_local(),
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => warn!(
                "peer BYE TERMINATED CAS lost: stream_id={}; call_id={call_id}",
                session.stream_id
            ),
            Err(err) => error!(
                "persist peer BYE TERMINATED failed: stream_id={}; call_id={call_id}; err={err}",
                session.stream_id
            ),
        }
    }
}

fn stream_resp_unit(resp: Resp<()>, action: &str) -> GlobalResult<()> {
    let Resp { code, msg, .. } = resp;
    if code == 200 {
        Ok(())
    } else {
        Err(GlobalError::new_biz_error(code, &msg, |log_msg| {
            error!("{action} failed: {log_msg}")
        }))
    }
}

fn validate_playback_range(st: u32, et: u32) -> GlobalResult<()> {
    if st < et {
        Ok(())
    } else {
        Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "playback start time must be before end time",
            |msg| error!("{msg}: st={st}, et={et}"),
        ))
    }
}

fn playback_stream_device(stream_id: &String) -> GlobalResult<String> {
    let access_mode = state::session::Cache::stream_map_query_play_type_by_stream_id(stream_id)
        .ok_or_else(|| {
            GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "stream not found", |msg| {
                error!("{msg}: stream_id={stream_id}")
            })
        })?;
    if !matches!(access_mode, AccessMode::Back | AccessMode::Down) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "stream is not a playback stream",
            |msg| error!("{msg}: stream_id={stream_id}"),
        ));
    }
    state::session::Cache::stream_device_id(stream_id).ok_or_else(|| {
        GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "stream not found", |msg| {
            error!("{msg}: stream_id={stream_id}")
        })
    })
}

async fn start_invite_stream(
    device_id: &String,
    channel_id: &String,
    _token: &String,
    am: AccessMode,
    st: u32,
    et: u32,
    trans_mode: Option<TransMode>,
    custom_media_config: Option<CustomMediaConfig>,
) -> GlobalResult<(String, String, String)> {
    let u16ssrc = state::session::Cache::ssrc_sn_get().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::IoBusy.code(),
            "ssrc已用完,并发达上限,等待释放",
            |msg| error!("{msg}"),
        )
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
        let Some(stream_node) = conf.node_map.get(&node_name) else {
            warn!("stream node configuration not found: node={node_name}");
            continue;
        };
        let client =
            HttpClient::template_ip_port(&stream_node.local_ip.to_string(), stream_node.local_port)
                .hand_log(|msg| error!("{msg}"))?;

        let Ok(res) = client
            .stream_init(&msc)
            .await
            .hand_log(|msg| error!("{msg}"))
        else {
            continue;
        };
        if res.code != 200 {
            continue;
        }

        let invite_res = match am {
            AccessMode::Live => {
                sip_command::play_live_invite_wait(
                    device_id,
                    channel_id,
                    &node_name,
                    &stream_node.pub_ip.to_string(),
                    stream_node.pub_port,
                    trans_mode.unwrap_or(TransMode::Udp),
                    &ssrc,
                    &stream_id,
                )
                .await
            }
            AccessMode::Back => {
                sip_command::play_back_invite_wait(
                    device_id,
                    channel_id,
                    &node_name,
                    &stream_node.pub_ip.to_string(),
                    stream_node.pub_port,
                    trans_mode.unwrap_or(TransMode::Udp),
                    &ssrc,
                    &stream_id,
                    st,
                    et,
                )
                .await
            }
            AccessMode::Down => {
                sip_command::download_invite_wait(
                    device_id,
                    channel_id,
                    &node_name,
                    &stream_node.pub_ip.to_string(),
                    stream_node.pub_port,
                    trans_mode.unwrap_or(TransMode::Udp),
                    &ssrc,
                    &stream_id,
                    st,
                    et,
                    1,
                )
                .await
            }
            AccessMode::Talk => unreachable!("talk does not use start_invite_stream"),
        };
        let (invite_accepted, media_ext) = match invite_res {
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
            .and_then(|resp| stream_resp_unit(resp.value(), "stream_init_ext"))
        {
            let _ = sip_command::invite_stop_by_device(
                device_id,
                crate::gb::sip::InviteStopRequest {
                    call_id: Some(invite_accepted.call_id.clone()),
                    stream_id: Some(stream_id.clone()),
                },
            )
            .await;
            cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
            state::session::Cache::ssrc_sn_set(u16ssrc);
            return Err(err);
        }

        let call_id = invite_accepted.call_id.clone();
        let seq = 1;
        if !state::session::Cache::stream_map_insert_info(
            stream_id.clone(),
            device_id.clone(),
            channel_id.clone(),
            u32ssrc,
            String::new(),
            node_name.clone(),
            call_id.clone(),
            seq,
            am,
        ) {
            let _ = sip_command::invite_stop_by_device(
                device_id,
                crate::gb::sip::InviteStopRequest {
                    call_id: Some(call_id),
                    stream_id: Some(stream_id.clone()),
                },
            )
            .await;
            cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
            state::session::Cache::ssrc_sn_set(u16ssrc);
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "stream dialog already exists",
                |msg| error!("{msg}: stream_id={stream_id}"),
            ));
        }

        if let Some(base_stream_info) = listen_stream_by_stream_id(&stream_id, EXPIRES).await {
            state::session::Cache::stream_map_update_source(
                &stream_id,
                base_stream_info.rtp_info.proxy_addr.clone(),
                node_name.clone(),
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

        cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
        stream_close::begin(stream_id);
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Timeout.code(),
            "未接收到监控推流",
            |msg| error!("{msg}"),
        ));
    }

    state::session::Cache::ssrc_sn_set(u16ssrc);
    Err(GlobalError::new_biz_error(
        BaseErrorCode::Network.code(),
        "无可用流媒体服务",
        |msg| error!("{msg}"),
    ))
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
                stream_close::begin(stream_id);
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
