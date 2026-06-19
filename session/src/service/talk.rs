use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use shared::info::obj::{TalkCloseReq, TalkStartModel};
use shared::info::res::Resp;

use crate::gb::sip::GbIncomingInviteEvent;
use crate::http::client::HttpStream;
use crate::state::model::TransMode;

const DEFAULT_TALK_CODEC: &str = "PCMA";
const DEFAULT_TALK_SAMPLE_RATE: u32 = 8000;
const DEFAULT_TALK_CHANNEL_COUNT: u8 = 1;
const DEFAULT_TALK_FRAME_DURATION_MS: u16 = 20;

pub(super) const DEFAULT_TALK_INPUT_TIMEOUT_SECS: u16 = 15;

pub(super) struct TalkAudioOptions {
    pub codec: String,
    pub payload_type: u8,
    pub sample_rate: u32,
    pub channel_count: u8,
    pub frame_duration_ms: u16,
    pub trans_mode: TransMode,
}

impl TalkAudioOptions {
    pub fn try_from_model(model: &TalkStartModel) -> GlobalResult<Self> {
        let codec_input = model.codec.as_deref().unwrap_or(DEFAULT_TALK_CODEC);
        let Some((codec, payload_type)) = normalize_talk_codec(codec_input) else {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "unsupported talk codec",
                |msg| error!("{msg}: {codec_input}"),
            ));
        };
        if codec != DEFAULT_TALK_CODEC {
            return Err(GlobalError::new_biz_error(
                BaseErrorCode::Unsupported.code(),
                "only PCMA talk audio is supported",
                |msg| error!("{msg}: codec={codec_input}"),
            ));
        }
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

    pub fn compatible_answer(&self, codec: &str, sample_rate: u32) -> bool {
        normalize_talk_codec(codec)
            .map(|(answer_codec, _)| answer_codec == self.codec && sample_rate == self.sample_rate)
            .unwrap_or(false)
    }
}

pub(super) struct TalkSdpAnswer {
    pub device_ip: String,
    pub device_port: u16,
    pub protocol: base::net::state::Protocol,
    pub payload_type: u8,
    pub codec: String,
    pub sample_rate: u32,
}

pub(super) fn parse_broadcast_invite(
    invite: &GbIncomingInviteEvent,
) -> GlobalResult<TalkSdpAnswer> {
    parse_broadcast_sdp(&invite.remote_sdp, &invite.call_id)
}

fn parse_broadcast_sdp(remote_sdp: &str, call_id: &str) -> GlobalResult<TalkSdpAnswer> {
    let sdp = gmv_pjsip::gb28181::sdp::SdpInfo::parse_lossy(remote_sdp);
    let has_audio = remote_sdp
        .lines()
        .map(str::trim)
        .any(|line| line.starts_with("m=audio "));
    let recvonly = remote_sdp
        .lines()
        .map(str::trim)
        .any(|line| line.eq_ignore_ascii_case("a=recvonly"));
    if sdp.session_name.as_deref() != Some("Play") || !has_audio || !recvonly {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "invalid broadcast INVITE SDP",
            |msg| error!("call_id={call_id}; {msg}"),
        ));
    }
    parse_talk_sdp(remote_sdp, &sdp)
}

fn parse_talk_sdp(
    remote_sdp: &str,
    sdp: &gmv_pjsip::gb28181::sdp::SdpInfo,
) -> GlobalResult<TalkSdpAnswer> {
    let device_ip = sdp.connection_addr.clone().ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "talk sdp missing audio connection address",
            |msg| error!("{msg}"),
        )
    })?;
    let device_port = sdp.media_port.ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "talk sdp missing audio media port",
            |msg| error!("{msg}"),
        )
    })?;
    let payload_type = sdp
        .media_payloads
        .first()
        .and_then(|value| value.parse::<u8>().ok())
        .unwrap_or(8);
    let (codec, sample_rate) =
        parse_rtpmap_from_sdp(remote_sdp, payload_type).unwrap_or_else(|| match payload_type {
            0 => ("PCMU".to_string(), 8000),
            8 => ("PCMA".to_string(), 8000),
            _ => (String::new(), 8000),
        });
    if codec.is_empty() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::Unsupported.code(),
            "unsupported talk payload type",
            |msg| error!("{msg}: pt={payload_type}"),
        ));
    }
    let protocol = sdp
        .media_proto
        .as_deref()
        .map(|proto| {
            if proto.to_ascii_uppercase().contains("TCP") {
                base::net::state::Protocol::TCP
            } else {
                base::net::state::Protocol::UDP
            }
        })
        .unwrap_or(base::net::state::Protocol::UDP);
    Ok(TalkSdpAnswer {
        device_ip,
        device_port,
        protocol,
        payload_type,
        codec,
        sample_rate,
    })
}

pub(super) fn stream_resp_data<T>(resp: Resp<T>, action: &str) -> GlobalResult<T> {
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

pub(super) fn append_gmv_token(input_url: String, token: &str) -> String {
    let encoded = url::form_urlencoded::byte_serialize(token.as_bytes()).collect::<String>();
    let sep = if input_url.contains('?') { '&' } else { '?' };
    format!("{input_url}{sep}gmv-token={encoded}")
}

pub(super) async fn cleanup_talk_open(client: &(impl HttpStream + ?Sized), talk_id: &str) {
    let _ = client
        .talk_close(&TalkCloseReq {
            talk_id: talk_id.to_string(),
        })
        .await
        .hand_log(|msg| error!("{msg}"));
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

fn parse_rtpmap_from_sdp(sdp: &str, payload_type: u8) -> Option<(String, u32)> {
    let prefix = format!("a=rtpmap:{payload_type}");
    for line in sdp.lines().map(str::trim) {
        let Some(rest) = line.strip_prefix(&prefix) else {
            continue;
        };
        let rest = rest.trim_start_matches([' ', ':']).trim();
        let mut parts = rest.split('/');
        let codec = parts.next()?.trim().to_uppercase();
        let sample_rate = parts
            .next()
            .and_then(|value| value.trim().parse::<u32>().ok())
            .unwrap_or(8000);
        return Some((codec, sample_rate));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::parse_broadcast_sdp;

    #[test]
    fn broadcast_invite_requires_play_audio_recvonly_pcma() {
        let valid = "v=0\r\ns=Play\r\nc=IN IP4 192.0.2.10\r\nt=0 0\r\nm=audio 30000 RTP/AVP 8\r\na=recvonly\r\na=rtpmap:8 PCMA/8000\r\n";
        let answer = parse_broadcast_sdp(valid, "call-1").expect("valid broadcast SDP");
        assert_eq!(answer.device_ip, "192.0.2.10");
        assert_eq!(answer.device_port, 30000);
        assert_eq!(answer.payload_type, 8);
        assert_eq!(answer.codec, "PCMA");
        assert_eq!(answer.sample_rate, 8000);

        assert!(parse_broadcast_sdp(&valid.replace("a=recvonly", "a=sendrecv"), "call-2").is_err());
        assert!(parse_broadcast_sdp(&valid.replace("s=Play", "s=Talk"), "call-3").is_err());
    }
}
