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
use shared::info::obj::{
    TalkAnswerReq, TalkCloseReq, TalkInfo, TalkOpenReq, TalkOpenResp, TalkStartModel, TalkStopModel,
};
use shared::info::output::{DashFmp4Output, LocalMp4Output, OutputEnum, OutputKind};
use shared::info::res::Resp;

use crate::gb::RWContext;
use crate::gb::handler::cmd::{CmdControl, CmdStream};
use crate::http::client::{HttpClient, HttpStream};
use crate::service::{EXPIRES, KEY_STREAM_IN};
use crate::state;
use crate::state::model::{
    CustomMediaConfig, PlayBackModel, PlayLiveModel, PlaySeekModel, PlaySpeedModel,
    PtzControlModel, StreamInfo, StreamQo, TransMode,
};
use crate::state::session::AccessMode;
use crate::state::session::TalkSessionState;
use crate::state::{DownloadConf, StreamConf, session};
use crate::storage::entity::GmvRecord;
use crate::utils::id_builder;

pub async fn play_live(play_live_model: PlayLiveModel, token: String) -> GlobalResult<StreamInfo> {
    let device_id = &play_live_model.device_id;
    if !RWContext::has_session_by_device_id(device_id) {
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
    let cst_info = state::session::Cache::stream_map_build_call_id_seq_from_to_tag(&stream_id);
    if let Ok((device_id, channel_id, ssrc_str)) = id_builder::de_stream_id(&stream_id) {
        let (stream_server, ssrc) = session::Cache::stream_map_query_node_ssrc(&stream_id)
            .ok_or_else(|| {
                GlobalError::new_biz_error(
                    BaseErrorCode::InvalidRequest.code(),
                    "无效的媒体流ID",
                    |msg| error!("{msg}"),
                )
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
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Network.code(),
            "设备已离线",
            |msg| error!("{msg}"),
        ));
    }
    let channel_id = play_back_model.channel_id.as_ref().unwrap_or(device_id);
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
                GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "流不存在", |msg| {
                    error!("{msg}")
                })
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
                GlobalError::new_biz_error(BaseErrorCode::NotFound.code(), "流不存在", |msg| {
                    error!("{msg}")
                })
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

pub async fn talk_start(model: TalkStartModel, token: String) -> GlobalResult<TalkInfo> {
    let device_id = &model.device_id;
    if !RWContext::has_session_by_device_id(device_id) {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Network.code(),
            "è®¾å¤‡å·²ç¦»çº¿",
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
            "ssrcå·²ç”¨å®Œ,å¹¶å‘è¾¾ä¸Šé™,ç­‰å¾…é‡Šæ”¾",
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

        let invite_res = CmdStream::talk_invite(
            device_id,
            &channel_id,
            &stream_node.pub_ip.to_string(),
            open_resp.rtp_port,
            audio.trans_mode,
            &ssrc,
            open_resp.payload_type,
            &open_resp.codec,
            open_resp.sample_rate,
        )
        .await;
        let (res, answer, from_tag, to_tag, association) = match invite_res {
            Ok(value) => value,
            Err(err) => {
                cleanup_talk_open(client.as_ref(), &talk_id).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };

        if !audio.compatible_answer(&answer.codec, answer.sample_rate) {
            if let Ok((call_id, seq)) = CmdStream::invite_ack(device_id, &res, association).await {
                let _ = CmdStream::play_bye(
                    seq.saturating_add(1),
                    call_id,
                    device_id,
                    &channel_id,
                    &from_tag,
                    &to_tag,
                )
                .await;
            }
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

        let (call_id, seq) = match CmdStream::invite_ack(device_id, &res, association).await {
            Ok(value) => value,
            Err(err) => {
                cleanup_talk_open(client.as_ref(), &talk_id).await;
                state::session::Cache::ssrc_sn_set(u16ssrc);
                return Err(err);
            }
        };

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
            let _ = CmdStream::play_bye(
                seq.saturating_add(1),
                call_id,
                device_id,
                &channel_id,
                &from_tag,
                &to_tag,
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
            call_id,
            seq,
            from_tag,
            to_tag,
            codec: open_resp.codec.clone(),
            sample_rate: open_resp.sample_rate,
            channel_count: open_resp.channel_count,
        };
        if !state::session::Cache::talk_map_insert(talk_state.clone()) {
            let _ = CmdStream::play_bye(
                talk_state.seq.saturating_add(1),
                talk_state.call_id,
                device_id,
                &channel_id,
                &talk_state.from_tag,
                &talk_state.to_tag,
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
        "æ— å¯ç”¨æµåª’ä½“æœåŠ¡",
        |msg| error!("{msg}"),
    ))
}

pub async fn talk_stop(model: TalkStopModel, _token: String) -> GlobalResult<bool> {
    let Some(talk) = state::session::Cache::talk_map_remove(&model.talk_id) else {
        return Ok(false);
    };

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

    let _ = CmdStream::play_bye(
        talk.seq.saturating_add(1),
        talk.call_id,
        &talk.device_id,
        &talk.channel_id,
        &talk.from_tag,
        &talk.to_tag,
    )
    .await;
    let ssrc_num = (talk.ssrc % 10000) as u16;
    state::session::Cache::ssrc_sn_set(ssrc_num);
    Ok(true)
}

const DEFAULT_TALK_CODEC: &str = "PCMA";
const DEFAULT_TALK_SAMPLE_RATE: u32 = 8000;
const DEFAULT_TALK_CHANNEL_COUNT: u8 = 1;
const DEFAULT_TALK_FRAME_DURATION_MS: u16 = 20;
const DEFAULT_TALK_INPUT_TIMEOUT_SECS: u16 = 15;

struct TalkAudioOptions {
    codec: String,
    payload_type: u8,
    sample_rate: u32,
    channel_count: u8,
    frame_duration_ms: u16,
    trans_mode: TransMode,
}

impl TalkAudioOptions {
    fn try_from_model(model: &TalkStartModel) -> GlobalResult<Self> {
        let codec_input = model.codec.as_deref().unwrap_or(DEFAULT_TALK_CODEC);
        let Some((codec, payload_type)) = normalize_talk_codec(codec_input) else {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "unsupported talk codec",
                |msg| error!("{msg}: {codec_input}"),
            ));
        };
        let sample_rate = model.sample_rate.unwrap_or(DEFAULT_TALK_SAMPLE_RATE);
        let channel_count = model.channel_count.unwrap_or(DEFAULT_TALK_CHANNEL_COUNT);
        let frame_duration_ms = model
            .frame_duration_ms
            .unwrap_or(DEFAULT_TALK_FRAME_DURATION_MS);
        let trans_mode = normalize_talk_transport(model.transport.as_deref())?;

        if sample_rate != DEFAULT_TALK_SAMPLE_RATE || channel_count != DEFAULT_TALK_CHANNEL_COUNT {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "only 8kHz mono talk audio is supported",
                |msg| error!("{msg}: sample_rate={sample_rate}, channel_count={channel_count}"),
            ));
        }
        if !(10..=60).contains(&frame_duration_ms)
            || sample_rate.saturating_mul(frame_duration_ms as u32) % 1000 != 0
        {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::InvalidRequest.code(),
                "invalid talk frame duration",
                |msg| error!("{msg}: frame_duration_ms={frame_duration_ms}"),
            ));
        }

        Ok(Self {
            codec: codec.to_string(),
            payload_type,
            sample_rate,
            channel_count,
            frame_duration_ms,
            trans_mode,
        })
    }

    fn compatible_answer(&self, codec: &str, sample_rate: u32) -> bool {
        normalize_talk_codec(codec)
            .map(|(answer_codec, _)| answer_codec == self.codec && sample_rate == self.sample_rate)
            .unwrap_or(false)
    }
}

fn normalize_talk_transport(transport: Option<&str>) -> GlobalResult<TransMode> {
    let Some(transport) = transport else {
        return Ok(TransMode::Udp);
    };
    let compact = transport
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    match compact.as_str() {
        "" | "UDP" => Ok(TransMode::Udp),
        "TCP" | "TCPPASSIVE" | "PASSIVE" => Ok(TransMode::TcpPassive),
        "TCPACTIVE" | "ACTIVE" => Err(GlobalError::new_biz_error(
            BaseErrorCode::Unsupported.code(),
            "tcp active talk is not supported",
            |msg| error!("{msg}: transport={transport}"),
        )),
        _ => Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidRequest.code(),
            "unsupported talk transport",
            |msg| error!("{msg}: transport={transport}"),
        )),
    }
}

fn normalize_talk_codec(codec: &str) -> Option<(&'static str, u8)> {
    let compact = codec
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_uppercase())
        .collect::<String>();
    match compact.as_str() {
        "PCMA" | "G711A" | "ALAW" => Some(("PCMA", 8)),
        "PCMU" | "G711U" | "MULAW" | "ULAW" => Some(("PCMU", 0)),
        _ => None,
    }
}

fn stream_resp_data<T>(resp: Resp<T>, action: &str) -> GlobalResult<T> {
    let Resp { code, msg, data } = resp;
    if code == 200 {
        data.ok_or_else(|| {
            GlobalError::new_biz_error(
                BaseErrorCode::InvalidState.code(),
                "stream response data is empty",
                |log_msg| error!("{action} failed: {log_msg}"),
            )
        })
    } else {
        Err(GlobalError::new_biz_error(code, &msg, |log_msg| {
            error!("{action} failed: {log_msg}")
        }))
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

fn append_gmv_token(input_url: String, token: &str) -> String {
    let encoded = url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>();
    let sep = if input_url.contains('?') { '&' } else { '?' };
    format!("{input_url}{sep}gmv-token={encoded}")
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
        let stream_node = conf.node_map.get(&node_name).unwrap();
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
            AccessMode::Talk => unreachable!("talk does not use start_invite_stream"),
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

        let (call_id, seq, dialog_target) = match CmdStream::invite_ack_with_dialog(
            device_id,
            &res,
            association,
        )
        .await
        {
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
                device_id.clone(),
                channel_id.clone(),
                u32ssrc,
                base_stream_info.rtp_info.proxy_addr.clone(),
                node_name.clone(),
                call_id,
                seq,
                am,
                from_tag,
                to_tag,
                dialog_target.remote_target,
                dialog_target.route_set,
                dialog_target.from_header,
                dialog_target.to_header,
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

        let _ = CmdStream::play_bye_dialog(
            seq + 1,
            call_id,
            device_id,
            &dialog_target.remote_target,
            &dialog_target.route_set,
            &dialog_target.from_header,
            &dialog_target.to_header,
        )
        .await;
        cleanup_stream_init(client.as_ref(), u32ssrc, &msc.output).await;
        state::session::Cache::ssrc_sn_set(u16ssrc);
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
                if let Ok((device_id, channel_id, _ssrc_str)) = id_builder::de_stream_id(&stream_id)
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

async fn cleanup_talk_open(client: &impl HttpStream, talk_id: &str) {
    let _ = client
        .talk_close(&TalkCloseReq {
            talk_id: talk_id.to_string(),
        })
        .await;
}
