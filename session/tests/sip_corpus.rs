use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use openssl::sha::sha256;

const PLATFORM_ID: &str = "34020000002000000001";
const DEVICE_ID: &str = "34020000001110000009";
const CHANNEL_ID: &str = "34020000001320000102";
const PLATFORM_ADDR: &str = "192.0.2.10:25600";
const DEVICE_ADDR: &str = "198.51.100.20:5060";

struct PacketAsset {
    scenario_id: &'static str,
    business_apis: &'static [&'static str],
    file_name: String,
    direction: &'static str,
    sip_method: &'static str,
    expected_status: Option<u16>,
    bytes: Vec<u8>,
}

fn request(
    method: &str,
    scenario_id: &str,
    cseq: u32,
    content_type: Option<&str>,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> Vec<u8> {
    let mut packet = format!(
        "{method} sip:{DEVICE_ID}@{DEVICE_ADDR} SIP/2.0\r\n\
Via: SIP/2.0/UDP {PLATFORM_ADDR};rport;branch=z9hG4bK-{scenario_id}\r\n\
Max-Forwards: 70\r\n\
From: <sip:{PLATFORM_ID}@3402000000>;tag=platform-{scenario_id}\r\n\
To: <sip:{DEVICE_ID}@{DEVICE_ADDR}>\r\n\
Contact: <sip:{PLATFORM_ID}@{PLATFORM_ADDR}>\r\n\
Call-ID: {scenario_id}@gmv.test\r\n\
CSeq: {cseq} {method}\r\n"
    );
    for (name, value) in extra_headers {
        packet.push_str(name);
        packet.push_str(": ");
        packet.push_str(value);
        packet.push_str("\r\n");
    }
    if let Some(content_type) = content_type {
        packet.push_str("Content-Type: ");
        packet.push_str(content_type);
        packet.push_str("\r\n");
    }
    packet.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    packet.into_bytes()
}

fn device_request(
    method: &str,
    scenario_id: &str,
    cseq: u32,
    content_type: Option<&str>,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> Vec<u8> {
    let mut packet = format!(
        "{method} sip:{PLATFORM_ID}@{PLATFORM_ADDR} SIP/2.0\r\n\
Via: SIP/2.0/UDP {DEVICE_ADDR};rport;branch=z9hG4bK-{scenario_id}\r\n\
Max-Forwards: 70\r\n\
From: <sip:{DEVICE_ID}@3402000000>;tag=device-{scenario_id}\r\n\
To: <sip:{PLATFORM_ID}@3402000000>\r\n\
Contact: <sip:{DEVICE_ID}@{DEVICE_ADDR}>\r\n\
Call-ID: {scenario_id}@gmv.test\r\n\
CSeq: {cseq} {method}\r\n"
    );
    for (name, value) in extra_headers {
        packet.push_str(name);
        packet.push_str(": ");
        packet.push_str(value);
        packet.push_str("\r\n");
    }
    if let Some(content_type) = content_type {
        packet.push_str("Content-Type: ");
        packet.push_str(content_type);
        packet.push_str("\r\n");
    }
    packet.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    packet.into_bytes()
}

#[allow(clippy::too_many_arguments)]
fn response(
    scenario_id: &str,
    cseq: u32,
    method: &str,
    status: u16,
    reason: &str,
    content_type: Option<&str>,
    body: &str,
    extra_headers: &[(&str, &str)],
) -> Vec<u8> {
    let mut packet = format!(
        "SIP/2.0 {status} {reason}\r\n\
Via: SIP/2.0/UDP {PLATFORM_ADDR};rport=25600;branch=z9hG4bK-{scenario_id}\r\n\
From: <sip:{PLATFORM_ID}@3402000000>;tag=platform-{scenario_id}\r\n\
To: <sip:{DEVICE_ID}@{DEVICE_ADDR}>;tag=device-{scenario_id}\r\n\
Call-ID: {scenario_id}@gmv.test\r\n\
CSeq: {cseq} {method}\r\n"
    );
    for (name, value) in extra_headers {
        packet.push_str(name);
        packet.push_str(": ");
        packet.push_str(value);
        packet.push_str("\r\n");
    }
    if let Some(content_type) = content_type {
        packet.push_str("Content-Type: ");
        packet.push_str(content_type);
        packet.push_str("\r\n");
    }
    packet.push_str(&format!("Content-Length: {}\r\n\r\n{body}", body.len()));
    packet.into_bytes()
}

fn response_to_device(
    scenario_id: &str,
    cseq: u32,
    method: &str,
    status: u16,
    reason: &str,
    extra_headers: &[(&str, &str)],
) -> Vec<u8> {
    let mut packet = format!(
        "SIP/2.0 {status} {reason}\r\n\
Via: SIP/2.0/UDP {DEVICE_ADDR};rport=5060;branch=z9hG4bK-{scenario_id}\r\n\
From: <sip:{DEVICE_ID}@3402000000>;tag=device-{scenario_id}\r\n\
To: <sip:{PLATFORM_ID}@3402000000>;tag=platform-{scenario_id}\r\n\
Call-ID: {scenario_id}@gmv.test\r\n\
CSeq: {cseq} {method}\r\n"
    );
    for (name, value) in extra_headers {
        packet.push_str(name);
        packet.push_str(": ");
        packet.push_str(value);
        packet.push_str("\r\n");
    }
    packet.push_str("Content-Length: 0\r\n\r\n");
    packet.into_bytes()
}

fn xml(root: &str, cmd_type: &str, sn: u32, device_id: &str, fields: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\r\n\
<{root}>\r\n\
<CmdType>{cmd_type}</CmdType>\r\n\
<SN>{sn}</SN>\r\n\
<DeviceID>{device_id}</DeviceID>\r\n\
{fields}</{root}>\r\n"
    )
}

fn push_exchange(
    assets: &mut Vec<PacketAsset>,
    scenario_id: &'static str,
    business_apis: &'static [&'static str],
    method: &'static str,
    cseq: u32,
    request_bytes: Vec<u8>,
) {
    assets.push(PacketAsset {
        scenario_id,
        business_apis,
        file_name: format!("{scenario_id}-01-request.sip"),
        direction: "platform-to-device",
        sip_method: method,
        expected_status: None,
        bytes: request_bytes,
    });
    assets.push(PacketAsset {
        scenario_id,
        business_apis,
        file_name: format!("{scenario_id}-02-200.sip"),
        direction: "device-to-platform",
        sip_method: method,
        expected_status: Some(200),
        bytes: response(scenario_id, cseq, method, 200, "OK", None, "", &[]),
    });
}

fn push_device_exchange(
    assets: &mut Vec<PacketAsset>,
    scenario_id: &'static str,
    method: &'static str,
    cseq: u32,
    request_bytes: Vec<u8>,
) {
    assets.push(PacketAsset {
        scenario_id,
        business_apis: &[],
        file_name: format!("{scenario_id}-01-request.sip"),
        direction: "device-to-platform",
        sip_method: method,
        expected_status: None,
        bytes: request_bytes,
    });
    assets.push(PacketAsset {
        scenario_id,
        business_apis: &[],
        file_name: format!("{scenario_id}-02-200.sip"),
        direction: "platform-to-device",
        sip_method: method,
        expected_status: Some(200),
        bytes: response_to_device(scenario_id, cseq, method, 200, "OK", &[]),
    });
}

fn push_invite(
    assets: &mut Vec<PacketAsset>,
    scenario_id: &'static str,
    business_apis: &'static [&'static str],
    cseq: u32,
    subject: &str,
    offer: String,
    answer: String,
) {
    assets.push(PacketAsset {
        scenario_id,
        business_apis,
        file_name: format!("{scenario_id}-01-invite.sip"),
        direction: "platform-to-device",
        sip_method: "INVITE",
        expected_status: None,
        bytes: request(
            "INVITE",
            scenario_id,
            cseq,
            Some("application/sdp"),
            &offer,
            &[("Subject", subject)],
        ),
    });
    assets.push(PacketAsset {
        scenario_id,
        business_apis,
        file_name: format!("{scenario_id}-02-100.sip"),
        direction: "device-to-platform",
        sip_method: "INVITE",
        expected_status: Some(100),
        bytes: response(scenario_id, cseq, "INVITE", 100, "Trying", None, "", &[]),
    });
    assets.push(PacketAsset {
        scenario_id,
        business_apis,
        file_name: format!("{scenario_id}-03-200.sip"),
        direction: "device-to-platform",
        sip_method: "INVITE",
        expected_status: Some(200),
        bytes: response(
            scenario_id,
            cseq,
            "INVITE",
            200,
            "OK",
            Some("application/sdp"),
            &answer,
            &[("Contact", &format!("<sip:{DEVICE_ID}@{DEVICE_ADDR}>"))],
        ),
    });
    assets.push(PacketAsset {
        scenario_id,
        business_apis,
        file_name: format!("{scenario_id}-04-ack.sip"),
        direction: "platform-to-device",
        sip_method: "ACK",
        expected_status: None,
        bytes: request("ACK", scenario_id, cseq, None, "", &[]),
    });
}

fn packet_assets() -> Vec<PacketAsset> {
    let mut assets = Vec::new();

    let register = device_request(
        "REGISTER",
        "register-normal",
        1,
        None,
        "",
        &[
            ("Expires", "3600"),
            ("User-Agent", "GMV-Synthetic-Device/1.0"),
            ("X-GB-Ver", "3.0"),
        ],
    );
    assets.push(PacketAsset {
        scenario_id: "register-normal",
        business_apis: &[],
        file_name: "register-normal-01-request.sip".into(),
        direction: "device-to-platform",
        sip_method: "REGISTER",
        expected_status: None,
        bytes: register,
    });
    assets.push(PacketAsset {
        scenario_id: "register-normal",
        business_apis: &[],
        file_name: "register-normal-02-200.sip".into(),
        direction: "platform-to-device",
        sip_method: "REGISTER",
        expected_status: Some(200),
        bytes: response_to_device(
            "register-normal",
            1,
            "REGISTER",
            200,
            "OK",
            &[("Expires", "3600"), ("X-GB-Ver", "3.0")],
        ),
    });

    let options = device_request("OPTIONS", "options-normal", 2, None, "", &[]);
    push_device_exchange(&mut assets, "options-normal", "OPTIONS", 2, options);

    let keepalive = xml(
        "Notify",
        "Keepalive",
        3,
        DEVICE_ID,
        "<Status>OK</Status>\r\n",
    );
    let keepalive_request = device_request(
        "MESSAGE",
        "keepalive-normal",
        3,
        Some("Application/MANSCDP+xml"),
        &keepalive,
        &[],
    );
    push_device_exchange(
        &mut assets,
        "keepalive-normal",
        "MESSAGE",
        3,
        keepalive_request,
    );

    let message_scenarios = [
        (
            "device-info-normal",
            &[][..],
            xml("Query", "DeviceInfo", 10, DEVICE_ID, ""),
        ),
        (
            "catalog-normal",
            &[][..],
            xml("Query", "Catalog", 11, DEVICE_ID, ""),
        ),
        (
            "record-info-normal",
            &[][..],
            xml(
                "Query",
                "RecordInfo",
                12,
                DEVICE_ID,
                "<StartTime>2026-06-13T00:00:00</StartTime>\r\n\
<EndTime>2026-06-13T01:00:00</EndTime>\r\n",
            ),
        ),
        (
            "preset-normal",
            &[][..],
            xml("Query", "PresetQuery", 13, CHANNEL_ID, ""),
        ),
        (
            "ptz-normal",
            &["/api/control/ptz"][..],
            xml(
                "Control",
                "DeviceControl",
                14,
                CHANNEL_ID,
                "<PTZCmd>A50F0102201000E7</PTZCmd>\r\n",
            ),
        ),
        (
            "snapshot-normal",
            &["/edge/snapshot/image", "/edge/upload/picture/{token}"][..],
            xml(
                "Control",
                "DeviceConfig",
                15,
                CHANNEL_ID,
                "<SnapShotConfig>\r\n\
<SnapNum>1</SnapNum>\r\n\
<Interval>1</Interval>\r\n\
<UploadURL>http://192.0.2.10:8080/edge/upload/picture/token</UploadURL>\r\n\
<SessionID>snapshot-session</SessionID>\r\n\
</SnapShotConfig>\r\n",
            ),
        ),
    ];
    for (index, (scenario_id, apis, body)) in message_scenarios.into_iter().enumerate() {
        let cseq = 10 + u32::try_from(index).expect("message index fits u32");
        let packet = request(
            "MESSAGE",
            scenario_id,
            cseq,
            Some("Application/MANSCDP+xml"),
            &body,
            &[],
        );
        push_exchange(&mut assets, scenario_id, apis, "MESSAGE", cseq, packet);
    }

    let answer_video = format!(
        "v=0\r\n\
o={DEVICE_ID} 0 0 IN IP4 198.51.100.20\r\n\
s=Play\r\n\
c=IN IP4 198.51.100.20\r\n\
t=0 0\r\n\
m=video 30000 RTP/AVP 96\r\n\
a=sendonly\r\n\
a=rtpmap:96 PS/90000\r\n\
y=0100008199\r\n"
    );
    let live_offer = format!(
        "v=0\r\n\
o={CHANNEL_ID} 0 0 IN IP4 192.0.2.10\r\n\
s=Play\r\n\
c=IN IP4 192.0.2.10\r\n\
t=0 0\r\n\
m=video 18568 RTP/AVP 96 97 98 99 100\r\n\
a=recvonly\r\n\
a=rtpmap:96 PS/90000\r\n\
a=rtpmap:97 MPEG4/90000\r\n\
a=rtpmap:98 H264/90000\r\n\
a=rtpmap:99 SVAC/90000\r\n\
a=rtpmap:100 H265/90000\r\n\
y=0100008199\r\n"
    );
    push_invite(
        &mut assets,
        "live-normal",
        &[
            "/api/play/live/stream",
            "/hook/stream/register",
            "/hook/on/play",
            "/hook/off/play",
        ],
        20,
        &format!("{CHANNEL_ID}:8199,{PLATFORM_ID}:0"),
        live_offer,
        answer_video.clone(),
    );
    let playback_offer = format!(
        "v=0\r\n\
o={CHANNEL_ID} 0 0 IN IP4 192.0.2.10\r\n\
s=Playback\r\n\
u={CHANNEL_ID}:0\r\n\
c=IN IP4 192.0.2.10\r\n\
t=1781308800 1781312400\r\n\
m=video 18568 RTP/AVP 96\r\n\
a=recvonly\r\n\
a=rtpmap:96 PS/90000\r\n\
y=0100008200\r\n"
    );
    push_invite(
        &mut assets,
        "playback-normal",
        &[
            "/api/play/back/stream",
            "/api/play/back/seek",
            "/api/play/back/speed",
        ],
        21,
        &format!("{CHANNEL_ID}:8200,{PLATFORM_ID}:0"),
        playback_offer,
        answer_video.clone(),
    );
    let download_offer = format!(
        "v=0\r\n\
o={CHANNEL_ID} 0 0 IN IP4 192.0.2.10\r\n\
s=Download\r\n\
u={CHANNEL_ID}:0\r\n\
c=IN IP4 192.0.2.10\r\n\
t=1781308800 1781312400\r\n\
m=video 18568 RTP/AVP 96\r\n\
a=recvonly\r\n\
a=rtpmap:96 PS/90000\r\n\
a=downloadspeed:1\r\n\
y=0100008201\r\n"
    );
    push_invite(
        &mut assets,
        "download-normal",
        &[
            "/api/download/mp4",
            "/api/download/stop",
            "/api/downing/info",
            "/api/rm/file",
            "/hook/end/record",
        ],
        22,
        &format!("{CHANNEL_ID}:8201,{PLATFORM_ID}:0"),
        download_offer,
        answer_video,
    );
    let talk_offer = format!(
        "v=0\r\n\
o={CHANNEL_ID} 0 0 IN IP4 192.0.2.10\r\n\
s=Talk\r\n\
c=IN IP4 192.0.2.10\r\n\
t=0 0\r\n\
m=audio 18570 RTP/AVP 8\r\n\
a=sendrecv\r\n\
a=rtpmap:8 PCMA/8000\r\n\
y=0200008202\r\n"
    );
    let talk_answer = format!(
        "v=0\r\n\
o={DEVICE_ID} 0 0 IN IP4 198.51.100.20\r\n\
s=Talk\r\n\
c=IN IP4 198.51.100.20\r\n\
t=0 0\r\n\
m=audio 30002 RTP/AVP 8\r\n\
a=sendrecv\r\n\
a=rtpmap:8 PCMA/8000\r\n\
y=0200008202\r\n"
    );
    push_invite(
        &mut assets,
        "talk-normal",
        &["/api/talk/start", "/api/talk/stop"],
        23,
        &format!("{CHANNEL_ID}:8202,{PLATFORM_ID}:0"),
        talk_offer,
        talk_answer,
    );

    let seek = "PLAY RTSP/1.0\r\nCSeq: 1\r\nRange: npt=30.000-\r\n\r\n";
    push_exchange(
        &mut assets,
        "seek-normal",
        &["/api/play/back/seek"],
        "INFO",
        24,
        request(
            "INFO",
            "seek-normal",
            24,
            Some("Application/MANSRTSP"),
            seek,
            &[],
        ),
    );
    let speed = "PLAY RTSP/1.0\r\nCSeq: 2\r\nScale: 2.000\r\n\r\n";
    push_exchange(
        &mut assets,
        "speed-normal",
        &["/api/play/back/speed"],
        "INFO",
        25,
        request(
            "INFO",
            "speed-normal",
            25,
            Some("Application/MANSRTSP"),
            speed,
            &[],
        ),
    );
    push_exchange(
        &mut assets,
        "bye-normal",
        &[
            "/api/download/stop",
            "/api/talk/stop",
            "/hook/stream/input/timeout",
            "/hook/stream/idle",
            "/hook/talk/closed",
        ],
        "BYE",
        26,
        request("BYE", "bye-normal", 26, None, "", &[]),
    );

    let subscribe_body = xml("Query", "Catalog", 27, DEVICE_ID, "");
    push_exchange(
        &mut assets,
        "subscribe-normal",
        &[],
        "SUBSCRIBE",
        27,
        request(
            "SUBSCRIBE",
            "subscribe-normal",
            27,
            Some("Application/MANSCDP+xml"),
            &subscribe_body,
            &[("Event", "Catalog"), ("Expires", "300")],
        ),
    );
    let notify_body = xml("Notify", "Catalog", 28, DEVICE_ID, "<SumNum>0</SumNum>\r\n");
    let notify = device_request(
        "NOTIFY",
        "subscribe-normal",
        28,
        Some("Application/MANSCDP+xml"),
        &notify_body,
        &[
            ("Event", "Catalog"),
            ("Subscription-State", "active;expires=299"),
        ],
    );
    assets.push(PacketAsset {
        scenario_id: "notify-normal",
        business_apis: &[],
        file_name: "notify-normal-01-request.sip".into(),
        direction: "device-to-platform",
        sip_method: "NOTIFY",
        expected_status: None,
        bytes: notify,
    });
    assets.push(PacketAsset {
        scenario_id: "notify-normal",
        business_apis: &[],
        file_name: "notify-normal-02-200.sip".into(),
        direction: "platform-to-device",
        sip_method: "NOTIFY",
        expected_status: Some(200),
        bytes: response_to_device("subscribe-normal", 28, "NOTIFY", 200, "OK", &[]),
    });

    assets
}

fn hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(output, "{byte:02x}").expect("write SHA-256");
    }
    output
}

fn manifest(assets: &[PacketAsset]) -> String {
    let scenario_count = assets
        .iter()
        .map(|asset| asset.scenario_id)
        .collect::<BTreeSet<_>>()
        .len();
    let covered_api_count = assets
        .iter()
        .flat_map(|asset| asset.business_apis.iter().copied())
        .collect::<BTreeSet<_>>()
        .len();
    let mut output = String::from(
        "version: 1\n\
source: synthetic-wire\n\
derived_from: 2026-06-13-live-invite-log\n\
sanitization: rfc5737-addresses-and-fixed-gb28181-identifiers\n\
generated_by: session/tests/sip_corpus.rs\n\
integrity_test: generated_sip_corpus_is_current_and_complete\n\
runtime_test: normal_gb28181_business_dialogues_use_custom_transport\n\
business_flow_test: all_business_http_apis_complete_the_normal_signaling_flow\n\
quality:\n",
    );
    writeln!(output, "  scenario_count: {scenario_count}").expect("write quality summary");
    writeln!(output, "  packet_count: {}", assets.len()).expect("write quality summary");
    writeln!(output, "  covered_business_api_count: {covered_api_count}")
        .expect("write quality summary");
    output.push_str("  failed_scenario_ids: []\n");
    output.push_str("  uncovered_business_apis: []\n");
    output.push_str("packets:\n");
    for asset in assets {
        let status = asset
            .expected_status
            .map_or_else(|| "null".to_string(), |value| value.to_string());
        writeln!(output, "  - scenario_id: {}", asset.scenario_id).expect("write manifest");
        writeln!(output, "    file: {}", asset.file_name).expect("write manifest");
        writeln!(output, "    direction: {}", asset.direction).expect("write manifest");
        writeln!(output, "    transport: udp").expect("write manifest");
        writeln!(output, "    sip_method: {}", asset.sip_method).expect("write manifest");
        writeln!(output, "    expected_status: {status}").expect("write manifest");
        writeln!(output, "    source: synthetic-wire").expect("write manifest");
        writeln!(output, "    sha256: {}", hex(&sha256(&asset.bytes))).expect("write manifest");
        if asset.business_apis.is_empty() {
            writeln!(output, "    business_apis: []").expect("write manifest");
        } else {
            writeln!(output, "    business_apis:").expect("write manifest");
            for api in asset.business_apis {
                writeln!(output, "      - {api}").expect("write manifest");
            }
        }
    }
    output
}

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sip")
        .join("generated")
}

fn assert_wire_packet(asset: &PacketAsset) {
    assert!(
        asset.bytes.windows(2).any(|pair| pair == b"\r\n"),
        "{} has no CRLF",
        asset.file_name
    );
    for (index, byte) in asset.bytes.iter().enumerate() {
        if *byte == b'\n' {
            assert!(
                index > 0 && asset.bytes[index - 1] == b'\r',
                "{} contains a bare LF",
                asset.file_name
            );
        }
    }
    let split = asset
        .bytes
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("packet contains header/body separator");
    let header = String::from_utf8_lossy(&asset.bytes[..split]);
    let body = &asset.bytes[split + 4..];
    let start_line = header.lines().next().expect("packet contains start line");
    if let Some(status) = asset.expected_status {
        assert!(
            start_line.starts_with(&format!("SIP/2.0 {status} ")),
            "{} response status mismatch",
            asset.file_name
        );
    } else {
        assert!(
            start_line.starts_with(&format!("{} ", asset.sip_method)),
            "{} request method mismatch",
            asset.file_name
        );
    }
    let declared = header
        .lines()
        .find_map(|line| {
            line.strip_prefix("Content-Length:")
                .map(str::trim)
                .and_then(|value| value.parse::<usize>().ok())
        })
        .expect("packet contains valid Content-Length");
    assert_eq!(
        declared,
        body.len(),
        "{} Content-Length mismatch",
        asset.file_name
    );
    for required in ["Via:", "From:", "To:", "Call-ID:", "CSeq:"] {
        assert!(
            header.contains(required),
            "{} missing {required}",
            asset.file_name
        );
    }
    let cseq = header
        .lines()
        .find_map(|line| line.strip_prefix("CSeq:"))
        .expect("packet contains CSeq");
    assert!(
        cseq.trim().ends_with(asset.sip_method),
        "{} CSeq method mismatch",
        asset.file_name
    );
    if !body.is_empty() {
        let lower_header = header.to_ascii_lowercase();
        assert!(
            lower_header.contains("content-type:"),
            "{} body has no Content-Type",
            asset.file_name
        );
        if lower_header.contains("xml") {
            assert!(
                body.starts_with(b"<?xml"),
                "{} XML body has no declaration",
                asset.file_name
            );
        }
        if lower_header.contains("sdp") {
            assert!(
                body.starts_with(b"v=0\r\n") && body.windows(4).any(|part| part == b"\r\nm="),
                "{} SDP body is incomplete",
                asset.file_name
            );
        }
    }
}

#[test]
fn generated_sip_corpus_is_current_and_complete() {
    let assets = packet_assets();
    assert!(assets.len() >= 40, "normal corpus packet count regressed");
    for asset in &assets {
        assert_wire_packet(asset);
    }

    let dir = fixture_dir();
    if std::env::var_os("GMV_UPDATE_SIP_CORPUS").is_some() {
        fs::create_dir_all(&dir).expect("create generated SIP fixture directory");
        for asset in &assets {
            fs::write(dir.join(&asset.file_name), &asset.bytes).expect("write SIP fixture");
        }
        fs::write(dir.join("manifest.yaml"), manifest(&assets)).expect("write SIP manifest");
    }

    for asset in &assets {
        let actual = fs::read(dir.join(&asset.file_name))
            .unwrap_or_else(|error| panic!("read {}: {error}", asset.file_name));
        assert_eq!(actual, asset.bytes, "{} is stale", asset.file_name);
    }
    let actual_manifest = fs::read_to_string(dir.join("manifest.yaml")).expect("read manifest");
    assert_eq!(actual_manifest, manifest(&assets), "manifest.yaml is stale");
    let mut expected_files = assets
        .iter()
        .map(|asset| asset.file_name.clone())
        .collect::<BTreeSet<_>>();
    expected_files.insert("manifest.yaml".into());
    let actual_files = fs::read_dir(&dir)
        .expect("read generated SIP fixture directory")
        .map(|entry| {
            entry
                .expect("read generated SIP fixture entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_files, expected_files,
        "generated SIP fixture directory contains stale or missing files"
    );

    let manifest_text = actual_manifest;
    for api in [
        "/api/play/live/stream",
        "/api/play/back/stream",
        "/api/play/back/seek",
        "/api/play/back/speed",
        "/api/control/ptz",
        "/api/download/mp4",
        "/api/download/stop",
        "/api/downing/info",
        "/api/rm/file",
        "/api/talk/start",
        "/api/talk/stop",
        "/edge/snapshot/image",
        "/edge/upload/picture/{token}",
        "/hook/stream/register",
        "/hook/stream/input/timeout",
        "/hook/on/play",
        "/hook/off/play",
        "/hook/stream/idle",
        "/hook/end/record",
        "/hook/talk/closed",
    ] {
        assert!(manifest_text.contains(api), "manifest missing API {api}");
    }
    let scenario_count = assets
        .iter()
        .map(|asset| asset.scenario_id)
        .collect::<BTreeSet<_>>()
        .len();
    println!(
        "sip corpus quality: scenarios={scenario_count}, packets={}, passed={}, \
failed_scenario_ids=[], uncovered_business_apis=[]",
        assets.len(),
        assets.len()
    );
}
