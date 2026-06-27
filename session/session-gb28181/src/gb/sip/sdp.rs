use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use gmv_domain::info::media_info_ext::MediaExt;
use regex::Regex;

use crate::gb::SessionConf;
use crate::state::model::TransMode;

pub use gmv_pjsip::gb28181::sdp::{PlaySdpOptions, SdpInfo, build_play_sdp};

pub fn video_payloads(support_h265: bool) -> &'static str {
    if support_h265 {
        "96 97 98 99 100"
    } else {
        "96 97 98 99"
    }
}

pub fn play_live(
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    stream_mode: TransMode,
    ssrc: &str,
    support_h265: bool,
) -> String {
    build_common_play(
        channel_id,
        media_ip,
        media_port,
        stream_mode,
        ssrc,
        "Play",
        "0 0",
        false,
        None,
        support_h265,
    )
}

pub fn playback(
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    stream_mode: TransMode,
    ssrc: &str,
    st: u32,
    et: u32,
    support_h265: bool,
) -> String {
    build_common_play(
        channel_id,
        media_ip,
        media_port,
        stream_mode,
        ssrc,
        "Playback",
        &format!("{} {}", st, et),
        true,
        None,
        support_h265,
    )
}

pub fn download(
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    stream_mode: TransMode,
    ssrc: &str,
    st: u32,
    et: u32,
    download_speed: u8,
    support_h265: bool,
) -> String {
    build_common_play(
        channel_id,
        media_ip,
        media_port,
        stream_mode,
        ssrc,
        "Download",
        &format!("{} {}", st, et),
        true,
        Some(download_speed),
        support_h265,
    )
}

fn build_common_play(
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    stream_mode: TransMode,
    ssrc: &str,
    name: &str,
    st_et: &str,
    add_u: bool,
    download_speed: Option<u8>,
    support_h265: bool,
) -> String {
    let conf = SessionConf::get_session_by_conf();
    let session_ip = conf.wan_ip.to_string();
    let payloads = video_payloads(support_h265);
    let mut sdp = String::with_capacity(320);
    sdp.push_str("v=0\r\n");
    sdp.push_str(&format!("o={} 0 0 IN IP4 {}\r\n", channel_id, session_ip));
    sdp.push_str(&format!("s={}\r\n", name));
    if add_u {
        sdp.push_str(&format!("u={}:0\r\n", channel_id));
    }
    sdp.push_str(&format!("c=IN IP4 {}\r\n", media_ip));
    sdp.push_str(&format!("t={}\r\n", st_et));
    match stream_mode {
        TransMode::Udp => sdp.push_str(&format!("m=video {} RTP/AVP {}\r\n", media_port, payloads)),
        TransMode::TcpActive => {
            sdp.push_str(&format!(
                "m=video {} TCP/RTP/AVP {}\r\n",
                media_port, payloads
            ));
            sdp.push_str("a=setup:active\r\n");
            sdp.push_str("a=connection:new\r\n");
        }
        TransMode::TcpPassive => {
            sdp.push_str(&format!(
                "m=video {} TCP/RTP/AVP {}\r\n",
                media_port, payloads
            ));
            sdp.push_str("a=setup:passive\r\n");
            sdp.push_str("a=connection:new\r\n");
        }
    }
    sdp.push_str("a=recvonly\r\n");
    sdp.push_str("a=rtpmap:96 PS/90000\r\n");
    sdp.push_str("a=rtpmap:97 MPEG4/90000\r\n");
    sdp.push_str("a=rtpmap:98 H264/90000\r\n");
    sdp.push_str("a=rtpmap:99 SVAC/90000\r\n");
    if support_h265 {
        sdp.push_str("a=rtpmap:100 H265/90000\r\n");
    }
    if let Some(speed) = download_speed {
        sdp.push_str(&format!("a=downloadspeed:{}\r\n", speed));
    }
    sdp.push_str(&format!("y={}\r\n", ssrc));
    sdp
}

pub fn parse_media_ext(sdp: &[u8]) -> GlobalResult<MediaExt> {
    let session = sdp_types::Session::parse(sdp).hand_log(|msg| error!("{msg}"))?;
    let re = Regex::new(r"\s+").hand_log(|msg| error!("{msg}"))?;
    let mut ext = MediaExt::default();
    for media in session.medias {
        if matches!(&*(media.media.trim().to_lowercase()), "video" | "audio") {
            if let Some(info) = media
                .get_first_attribute_value("rtpmap")
                .hand_log(|msg| error!("{msg}"))?
            {
                let trimmed = re.replace_all(info, " ").trim().to_string();
                if let Some((play_code, payload)) = trimmed.split_once(' ') {
                    let type_code: u8 = play_code.trim().parse().hand_log(|msg| error!("{msg}"))?;
                    ext.type_code = type_code;
                    let vs: Vec<&str> = payload.trim().split('/').collect();
                    if vs.len() >= 2 {
                        ext.type_name = vs[0].trim().to_uppercase();
                        ext.clock_rate = vs[1].trim().parse().hand_log(|msg| error!("{msg}"))?;
                    }
                }
            }
            if let Ok(Some(num)) = media.get_first_attribute_value("streamnumber") {
                ext.stream_number = Some(num.trim().parse().hand_log(|msg| error!("{msg}"))?);
            }
        }
    }
    extract_f_field(&mut ext, sdp);
    Ok(ext)
}

pub fn validate_invite_answer_sdp(remote_sdp: &str, expected_ssrc: &str) -> GlobalResult<()> {
    let info = SdpInfo::parse_lossy(remote_sdp);
    let Some(actual_ssrc) = info.ssrc.as_deref() else {
        return Err(invalid_answer_sdp("missing y= SSRC", expected_ssrc, None));
    };
    if actual_ssrc.len() != 10 || !actual_ssrc.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid_answer_sdp(
            "invalid y= SSRC format",
            expected_ssrc,
            Some(actual_ssrc),
        ));
    }
    if actual_ssrc != expected_ssrc {
        return Err(invalid_answer_sdp(
            "y= SSRC mismatch",
            expected_ssrc,
            Some(actual_ssrc),
        ));
    }
    if info
        .connection_addr
        .as_deref()
        .unwrap_or_default()
        .is_empty()
    {
        return Err(invalid_answer_sdp(
            "missing media connection address",
            expected_ssrc,
            Some(actual_ssrc),
        ));
    }
    if info.media_port.unwrap_or_default() == 0 {
        return Err(invalid_answer_sdp(
            "missing media port",
            expected_ssrc,
            Some(actual_ssrc),
        ));
    }
    if info.media_payloads.is_empty() {
        return Err(invalid_answer_sdp(
            "missing media payload",
            expected_ssrc,
            Some(actual_ssrc),
        ));
    }
    Ok(())
}

fn invalid_answer_sdp(reason: &str, expected_ssrc: &str, actual_ssrc: Option<&str>) -> GlobalError {
    GlobalError::new_biz_error(
        BaseErrorCode::InvalidState.code(),
        "invalid device SDP answer",
        |msg| {
            error!(
                "{msg}: reason={reason}; expected_ssrc={expected_ssrc}; actual_ssrc={}",
                actual_ssrc.unwrap_or("<missing>")
            )
        },
    )
}

fn extract_f_field(me: &mut MediaExt, sdp: &[u8]) {
    let Ok(sdp) = std::str::from_utf8(sdp) else {
        return;
    };
    if let Some(f_field) = sdp.lines().find_map(|line| line.trim().strip_prefix("f=")) {
        let parts: Vec<&str> = f_field.split('/').map(|item| item.trim()).collect();
        if parts.len() == 9 && parts[0] == "v" && parts[5].ends_with('a') {
            if !parts[1].is_empty() {
                me.video_params.map_video_codec(parts[1]);
            }
            if !parts[2].is_empty() {
                me.video_params.map_resolution(parts[2]);
            }
            if !parts[3].is_empty() {
                me.video_params.map_fps(parts[3]);
            }
            if !parts[4].is_empty() {
                me.video_params.map_bitrate_type(parts[4]);
            }
            let p5 = parts[5].strip_suffix('a').unwrap_or(parts[5]).trim();
            if !p5.is_empty() {
                me.video_params.map_bitrate(p5);
            }
            if !parts[6].is_empty() {
                me.audio_params.map_audio_codec(parts[6]);
            }
            if !parts[7].is_empty() {
                me.audio_params.map_bitrate(parts[7]);
            }
            if !parts[8].is_empty() {
                me.audio_params.map_sample_rate(parts[8]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::validate_invite_answer_sdp;

    const VALID_VIDEO_ANSWER: &str = "v=0\r\n\
o=34020000001320000001 0 0 IN IP4 198.51.100.20\r\n\
s=Play\r\n\
c=IN IP4 198.51.100.20\r\n\
t=0 0\r\n\
m=video 30000 RTP/AVP 96\r\n\
a=sendonly\r\n\
a=rtpmap:96 PS/90000\r\n\
y=0100008199\r\n";

    #[test]
    fn invite_answer_requires_matching_y_ssrc() {
        assert!(validate_invite_answer_sdp(VALID_VIDEO_ANSWER, "0100008199").is_ok());
        assert!(validate_invite_answer_sdp(VALID_VIDEO_ANSWER, "0100008200").is_err());
    }

    #[test]
    fn invite_answer_rejects_missing_y_ssrc() {
        let without_y = VALID_VIDEO_ANSWER.replace("y=0100008199\r\n", "");
        assert!(validate_invite_answer_sdp(&without_y, "0100008199").is_err());
    }
}
