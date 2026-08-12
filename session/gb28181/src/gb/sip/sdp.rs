use base::err::BaseErrorCode;
use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::error;
use gmv_domain::info::media_info_ext::{MediaDeclarationState, MediaExt, MediaSectionDeclaration};
use gmv_protocol::common::v1::{Endpoint, EndpointMode};
use regex::Regex;
use std::collections::HashMap;
use std::net::IpAddr;

use crate::gb::SessionConf;
use crate::state::model::{LiveStreamProfile, TransMode};

pub use gmv_pjsip::gb28181::sdp::{PlaySdpOptions, SdpInfo, build_play_sdp};

pub fn video_payloads(support_h265: bool) -> &'static str {
    if support_h265 {
        "96 97 98 99 100"
    } else {
        "96 97 98 99"
    }
}

pub fn supports_gb_2022(gb_version: Option<&str>) -> bool {
    gb_version.is_some_and(|version| version.trim() == "3.0")
}

pub fn uses_gb_2022_extension(media_ext: &MediaExt) -> bool {
    media_ext
        .video_params
        .codec_id
        .as_deref()
        .is_some_and(|codec| codec.eq_ignore_ascii_case("h265"))
        || media_ext
            .audio_params
            .codec_id
            .as_deref()
            .is_some_and(|codec| matches!(codec.to_ascii_lowercase().as_str(), "svac" | "aac"))
}

fn canonical_audio_codec(codec: &str) -> String {
    match codec.trim().to_ascii_uppercase().as_str() {
        "PCMA" | "G711A" | "G.711A" => "PCMA",
        "PCMU" | "G711U" | "G.711U" => "PCMU",
        "G723" | "G7231" | "G.723" | "G.723.1" => "G7231",
        "G729" | "G.729" => "G729",
        "G7221" | "G.722.1" | "SIREN" | "SIREN7" | "SIREN14" => "G7221",
        "MPEG4-GENERIC" | "MPEG4_AAC" | "MPEG4-AAC" | "AAC" => "AAC",
        other => other,
    }
    .to_string()
}

fn static_audio_payload(payload_type: u8) -> Option<(&'static str, i32, i32)> {
    match payload_type {
        0 => Some(("PCMU", 8_000, 1)),
        8 => Some(("PCMA", 8_000, 1)),
        18 => Some(("G729", 8_000, 1)),
        _ => None,
    }
}

fn selected_rtpmap(media: &sdp_types::Media) -> Option<&str> {
    media.fmt.split_whitespace().find_map(|payload_type| {
        media.attributes.iter().find_map(|attribute| {
            if !attribute.attribute.eq_ignore_ascii_case("rtpmap") {
                return None;
            }
            let value = attribute.value.as_deref()?;
            let (mapped_payload, _) = value.trim().split_once(char::is_whitespace)?;
            (mapped_payload == payload_type).then_some(value)
        })
    })
}

pub fn play_live(
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    stream_mode: TransMode,
    ssrc: &str,
    support_h265: bool,
) -> String {
    play_live_with_profile(
        channel_id,
        media_ip,
        media_port,
        stream_mode,
        ssrc,
        LiveStreamProfile::Main,
        support_h265,
    )
}

pub fn play_live_with_profile(
    channel_id: &str,
    media_ip: &str,
    media_port: u16,
    stream_mode: TransMode,
    ssrc: &str,
    stream_profile: LiveStreamProfile,
    support_h265: bool,
) -> String {
    let mut sdp = build_common_play(
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
    );
    sdp = with_stream_number(sdp, ssrc, stream_profile);
    sdp
}

fn with_stream_number(sdp: String, ssrc: &str, stream_profile: LiveStreamProfile) -> String {
    let marker = format!("y={}\r\n", ssrc);
    sdp.replacen(
        &marker,
        &format!(
            "a=streamnumber:{}\r\n{}",
            stream_profile.stream_number(),
            marker
        ),
        1,
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
        let media_kind = media.media.trim().to_lowercase();
        if matches!(media_kind.as_str(), "video" | "audio") {
            let mut declaration = MediaSectionDeclaration {
                state: if media.port == 0 {
                    MediaDeclarationState::Rejected
                } else {
                    MediaDeclarationState::Active
                },
                ..Default::default()
            };
            if let Some(info) = selected_rtpmap(&media) {
                let trimmed = re.replace_all(info, " ").trim().to_string();
                if let Some((play_code, payload)) = trimmed.split_once(' ') {
                    let type_code: u8 = play_code.trim().parse().hand_log(|msg| error!("{msg}"))?;
                    declaration.payload_type = Some(type_code);
                    let values: Vec<&str> = payload.trim().split('/').collect();
                    if values.len() >= 2 {
                        let codec = if media_kind == "audio" {
                            canonical_audio_codec(values[0])
                        } else {
                            values[0].trim().to_uppercase()
                        };
                        let clock_rate = values[1]
                            .trim()
                            .parse::<i32>()
                            .hand_log(|msg| error!("{msg}"))?;
                        declaration.codec = Some(codec.clone());
                        declaration.clock_rate = Some(clock_rate);
                        declaration.channels = values.get(2).and_then(|value| value.parse().ok());
                        if media_kind == "video" || ext.type_name.is_empty() {
                            ext.type_code = type_code;
                            ext.type_name = codec;
                            ext.clock_rate = clock_rate;
                        }
                    }
                }
            } else {
                declaration.payload_type = media
                    .fmt
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok());
                if media_kind == "audio"
                    && let Some((codec, clock_rate, channels)) =
                        declaration.payload_type.and_then(static_audio_payload)
                {
                    declaration.codec = Some(codec.to_string());
                    declaration.clock_rate = Some(clock_rate);
                    declaration.channels = Some(channels);
                }
            }
            if media_kind == "video" {
                ext.declaration.video = declaration;
            } else {
                ext.declaration.audio = declaration;
            }
            if let Ok(Some(num)) = media.get_first_attribute_value("streamnumber") {
                ext.stream_number = Some(num.trim().parse().hand_log(|msg| error!("{msg}"))?);
            }
        }
    }
    extract_f_field(&mut ext, sdp);
    if ext.audio_params.codec_id.is_some()
        && ext.declaration.audio.state == MediaDeclarationState::Absent
    {
        ext.declaration.audio.state = MediaDeclarationState::Active;
        ext.declaration.audio.codec = ext.audio_params.codec_id.clone();
        ext.declaration.audio.clock_rate =
            (ext.audio_params.clock_rate > 0).then_some(ext.audio_params.clock_rate);
        ext.declaration.audio.channels =
            (ext.audio_params.channel_count > 0).then_some(ext.audio_params.channel_count);
        ext.declaration.audio.embedded_in_ps = true;
    }
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

pub fn remote_media_endpoint(remote_sdp: &str) -> GlobalResult<Endpoint> {
    let info = SdpInfo::parse_lossy(remote_sdp);
    let host = info.connection_addr.unwrap_or_default();
    let ip = host.parse::<IpAddr>().map_err(|_| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "media_peer_policy_required",
            |msg| error!("{msg}: invalid SDP connection address"),
        )
    })?;
    if ip.is_unspecified() || ip.is_multicast() {
        return Err(GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "media_peer_unreachable",
            |msg| error!("{msg}: remote_addr={ip}"),
        ));
    }
    let port = info.media_port.filter(|port| *port != 0).ok_or_else(|| {
        GlobalError::new_biz_error(
            BaseErrorCode::InvalidState.code(),
            "media_peer_policy_required",
            |msg| error!("{msg}: missing SDP media port"),
        )
    })?;
    Ok(Endpoint {
        name: "rtp-peer".to_string(),
        scheme: "rtp".to_string(),
        host,
        port: u32::from(port),
        mode: EndpointMode::Single as i32,
        labels: HashMap::from([("address_policy".to_string(), "sdp_exact".to_string())]),
    })
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
    use super::{
        parse_media_ext, remote_media_endpoint, supports_gb_2022, uses_gb_2022_extension,
        validate_invite_answer_sdp, video_payloads, with_stream_number,
    };
    use crate::state::model::LiveStreamProfile;
    use gmv_domain::info::media_info_ext::MediaDeclarationState;

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
    fn live_sdp_carries_requested_stream_profile() {
        let source = "v=0\r\ny=0100008199\r\n".to_string();
        let main = with_stream_number(source.clone(), "0100008199", LiveStreamProfile::Main);
        let sub = with_stream_number(source, "0100008199", LiveStreamProfile::Sub);
        assert!(main.contains("a=streamnumber:0\r\n"));
        assert!(sub.contains("a=streamnumber:1\r\n"));
    }

    #[test]
    fn invite_answer_rejects_missing_y_ssrc() {
        let without_y = VALID_VIDEO_ANSWER.replace("y=0100008199\r\n", "");
        assert!(validate_invite_answer_sdp(&without_y, "0100008199").is_err());
    }

    #[test]
    fn remote_media_endpoint_uses_explicit_sdp_address() {
        let endpoint = remote_media_endpoint(VALID_VIDEO_ANSWER).unwrap();
        assert_eq!(endpoint.host, "198.51.100.20");
        assert_eq!(endpoint.port, 30_000);
        assert_eq!(endpoint.labels["address_policy"], "sdp_exact");

        let unspecified = VALID_VIDEO_ANSWER.replace("198.51.100.20", "0.0.0.0");
        assert!(remote_media_endpoint(&unspecified).is_err());
    }

    #[test]
    fn media_ext_preserves_active_audio_declaration() {
        let sdp =
            format!("{VALID_VIDEO_ANSWER}m=audio 30002 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000/1\r\n");
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert_eq!(ext.declaration.video.state, MediaDeclarationState::Active);
        assert_eq!(ext.declaration.audio.state, MediaDeclarationState::Active);
        assert_eq!(ext.declaration.audio.payload_type, Some(8));
        assert_eq!(ext.declaration.audio.codec.as_deref(), Some("PCMA"));
        assert_eq!(ext.declaration.audio.clock_rate, Some(8_000));
        assert_eq!(ext.declaration.audio.channels, Some(1));
        assert!(!ext.declaration.audio.embedded_in_ps);
    }

    #[test]
    fn media_ext_preserves_rejected_audio_declaration() {
        let sdp = format!("{VALID_VIDEO_ANSWER}m=audio 0 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000/1\r\n");
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert_eq!(ext.declaration.audio.state, MediaDeclarationState::Rejected);
    }

    #[test]
    fn embedded_audio_from_f_field_is_declared_without_separate_media() {
        let sdp = format!("{VALID_VIDEO_ANSWER}f=v/2/5/25/1/8000a/1/8/1\r\n");
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert_eq!(ext.declaration.audio.state, MediaDeclarationState::Active);
        assert!(ext.declaration.audio.embedded_in_ps);
        assert_eq!(ext.declaration.audio.codec.as_deref(), Some("g711"));
        assert_eq!(ext.declaration.audio.clock_rate, Some(8_000));
        assert_eq!(ext.audio_params.bitrate.as_deref(), Some("64"));
    }

    #[test]
    fn static_audio_payloads_are_resolved_without_rtpmap() {
        for (payload, codec) in [(0, "PCMU"), (8, "PCMA"), (18, "G729")] {
            let sdp = format!("{VALID_VIDEO_ANSWER}m=audio 30002 RTP/AVP {payload}\r\n");
            let ext = parse_media_ext(sdp.as_bytes()).unwrap();

            assert_eq!(ext.declaration.audio.codec.as_deref(), Some(codec));
            assert_eq!(ext.declaration.audio.clock_rate, Some(8_000));
            assert_eq!(ext.declaration.audio.channels, Some(1));
        }
    }

    #[test]
    fn vendor_audio_rtpmap_names_are_canonicalized() {
        let sdp = format!(
            "{VALID_VIDEO_ANSWER}m=audio 30002 RTP/AVP 100\r\na=rtpmap:100 SIREN14/32000/1\r\n"
        );
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert_eq!(ext.declaration.audio.codec.as_deref(), Some("G7221"));
        assert_eq!(ext.declaration.audio.clock_rate, Some(32_000));
    }

    #[test]
    fn rtpmap_is_selected_by_media_payload_order() {
        let sdp = format!(
            "{VALID_VIDEO_ANSWER}m=audio 30002 RTP/AVP 8 100\r\na=rtpmap:100 SIREN14/32000/1\r\na=rtpmap:8 PCMA/8000/1\r\n"
        );
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert_eq!(ext.declaration.audio.payload_type, Some(8));
        assert_eq!(ext.declaration.audio.codec.as_deref(), Some("PCMA"));
    }

    #[test]
    fn f_field_codec_four_is_g7221_and_does_not_invent_channels() {
        let sdp = format!("{VALID_VIDEO_ANSWER}f=v/2/5/25/1/8000a/4/8/3\r\n");
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert_eq!(ext.declaration.audio.codec.as_deref(), Some("g7221"));
        assert_eq!(ext.declaration.audio.clock_rate, Some(16_000));
        assert_eq!(ext.declaration.audio.channels, None);
    }

    #[test]
    fn outbound_extended_video_requires_explicit_2022_registration() {
        assert!(!supports_gb_2022(None));
        assert!(!supports_gb_2022(Some("2.0")));
        assert!(!supports_gb_2022(Some("unknown")));
        assert!(supports_gb_2022(Some(" 3.0 ")));
        assert!(!video_payloads(false).contains("100"));
        assert!(video_payloads(true).contains("100"));
    }

    #[test]
    fn received_2022_f_values_are_detected_without_rejecting_the_sdp() {
        let sdp = format!("{VALID_VIDEO_ANSWER}f=v/5/5/25/1/8000a/6/8/11\r\n");
        let ext = parse_media_ext(sdp.as_bytes()).unwrap();

        assert!(uses_gb_2022_extension(&ext));
        assert_eq!(ext.video_params.codec_id.as_deref(), Some("h265"));
        assert_eq!(ext.audio_params.codec_id.as_deref(), Some("aac"));
    }
}
