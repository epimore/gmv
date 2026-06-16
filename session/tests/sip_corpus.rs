use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use openssl::sha::sha256;

const PLATFORM_ID: &str = "34020000002000000001";
const DEVICE_ID: &str = "34020000001110000009";
const CHANNEL_ID: &str = "34020000001320000102";
const PLATFORM_ADDR: &str = "192.0.2.10:25600";
const DEVICE_ADDR: &str = "198.51.100.20:5060";
const REFERENCE_SOURCE_SHA256: &str =
    "2d89bb70302b80f83e1aa9d8956c36d95ddb123c3f0bdf4c5c2519333319a262";
const REFERENCE_PACKET_COUNT: usize = 108;
const REFERENCE_TC_COUNT: usize = 26;
const REFERENCE_SDP_PACKET_COUNT: usize = 10;
const REFERENCE_SDP_Y_COUNT: usize = 10;
const REFERENCE_PLATFORM_ID: &str = "34020000002000000001";
const REFERENCE_PLATFORM_ADDR: &str = "192.168.10.10";

struct PacketAsset {
    scenario_id: &'static str,
    business_apis: &'static [&'static str],
    file_name: String,
    direction: &'static str,
    sip_method: &'static str,
    expected_status: Option<u16>,
    bytes: Vec<u8>,
}

struct ReferencePacket {
    tc_id: String,
    tc_title: String,
    file_name: String,
    direction: &'static str,
    sip_method: String,
    expected_status: Option<u16>,
    content_type: Option<String>,
    call_id: String,
    cseq: String,
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

fn reference_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("sip")
        .join("reference")
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

fn header_value<'a>(header: &'a str, name: &str) -> Option<&'a str> {
    header.lines().find_map(|line| {
        let (header_name, value) = line.split_once(':')?;
        header_name
            .eq_ignore_ascii_case(name)
            .then_some(value.trim())
    })
}

fn reference_body(packet: &[u8]) -> (&str, &[u8]) {
    let split = packet
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("reference packet contains header/body separator");
    (
        std::str::from_utf8(&packet[..split]).expect("reference header is UTF-8"),
        &packet[split + 4..],
    )
}

fn markdown_reference_packets(source: &str) -> Vec<ReferencePacket> {
    let mut packets = Vec::new();
    let mut current_title = String::new();
    let mut current_tc = String::new();
    let mut in_sip_block = false;
    let mut block_lines = Vec::new();

    for line in source.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(title) = line.strip_prefix("### TC-") {
            current_title = format!("TC-{title}");
            current_tc = current_title
                .split_whitespace()
                .next()
                .unwrap_or("TC-unknown")
                .to_ascii_lowercase();
            continue;
        }
        if line.trim() == "```sip" {
            in_sip_block = true;
            block_lines.clear();
            continue;
        }
        if in_sip_block && line.trim() == "```" {
            let bytes = block_lines.join("\r\n").into_bytes();
            let (header, body) = reference_body(&bytes);
            let start_line = header.lines().next().expect("reference start line");
            let cseq = header_value(header, "CSeq")
                .expect("reference packet has CSeq")
                .to_owned();
            let (sip_method, expected_status) =
                if let Some(rest) = start_line.strip_prefix("SIP/2.0 ") {
                    let status = rest
                        .split_whitespace()
                        .next()
                        .and_then(|value| value.parse::<u16>().ok())
                        .expect("reference response has status");
                    let method = cseq
                        .split_whitespace()
                        .last()
                        .expect("reference CSeq has method")
                        .to_owned();
                    (method, Some(status))
                } else {
                    let method = start_line
                        .split_whitespace()
                        .next()
                        .expect("reference request has method")
                        .to_owned();
                    (method, None)
                };
            let direction = reference_direction(header, expected_status.is_some());
            let content_type = header_value(header, "Content-Type").map(str::to_owned);
            let call_id = header_value(header, "Call-ID")
                .expect("reference packet has Call-ID")
                .to_owned();
            let packet_index = packets.len() + 1;
            let tc_id = if current_tc.is_empty() {
                "tc-unknown".to_owned()
            } else {
                current_tc.clone()
            };
            let file_name = format!("{tc_id}-{packet_index:03}.sip");
            assert!(
                body.is_empty() || content_type.is_some(),
                "{file_name} has body without Content-Type"
            );
            packets.push(ReferencePacket {
                tc_id,
                tc_title: current_title.clone(),
                file_name,
                direction,
                sip_method,
                expected_status,
                content_type,
                call_id,
                cseq,
                bytes,
            });
            in_sip_block = false;
            continue;
        }
        if in_sip_block {
            block_lines.push(line.to_owned());
        }
    }

    packets
}

fn reference_direction(header: &str, response: bool) -> &'static str {
    if !response {
        let from = header_value(header, "From").unwrap_or_default();
        return if from.contains(REFERENCE_PLATFORM_ID) {
            "platform-to-device"
        } else {
            "device-to-platform"
        };
    }
    let user_agent = header_value(header, "User-Agent").unwrap_or_default();
    if user_agent.contains("GMV-GB28181-Platform") {
        return "platform-to-device";
    }
    if user_agent.contains("IPC-GB28181-UA") {
        return "device-to-platform";
    }
    let via = header_value(header, "Via").unwrap_or_default();
    if via.contains(REFERENCE_PLATFORM_ADDR) {
        "device-to-platform"
    } else {
        "platform-to-device"
    }
}

fn assert_reference_packet(packet: &ReferencePacket) {
    assert!(
        packet.bytes.windows(2).any(|pair| pair == b"\r\n"),
        "{} has no CRLF",
        packet.file_name
    );
    for (index, byte) in packet.bytes.iter().enumerate() {
        if *byte == b'\n' {
            assert!(
                index > 0 && packet.bytes[index - 1] == b'\r',
                "{} contains a bare LF",
                packet.file_name
            );
        }
    }
    let (header, body) = reference_body(&packet.bytes);
    let start_line = header.lines().next().expect("reference start line");
    if let Some(status) = packet.expected_status {
        assert!(
            start_line.starts_with(&format!("SIP/2.0 {status} ")),
            "{} response status mismatch",
            packet.file_name
        );
    } else {
        assert!(
            start_line.starts_with(&format!("{} ", packet.sip_method)),
            "{} request method mismatch",
            packet.file_name
        );
        assert!(
            header_value(header, "Max-Forwards").is_some(),
            "{} request missing Max-Forwards",
            packet.file_name
        );
    }
    for required in ["Via", "From", "To", "Call-ID", "CSeq"] {
        assert!(
            header_value(header, required).is_some(),
            "{} missing {required}",
            packet.file_name
        );
    }
    let declared = header_value(header, "Content-Length")
        .and_then(|value| value.parse::<usize>().ok())
        .expect("reference packet contains valid Content-Length");
    assert_eq!(
        declared,
        body.len(),
        "{} Content-Length mismatch",
        packet.file_name
    );
    assert!(
        packet.cseq.ends_with(&packet.sip_method),
        "{} CSeq method mismatch",
        packet.file_name
    );
    if let Some(content_type) = &packet.content_type {
        let lower = content_type.to_ascii_lowercase();
        if lower.contains("xml") {
            assert!(
                body.starts_with(b"<?xml"),
                "{} XML body has no declaration",
                packet.file_name
            );
        }
        if lower.contains("sdp") {
            assert_sdp_has_valid_ssrc(packet, header, body);
        }
    } else {
        assert!(body.is_empty(), "{} missing Content-Type", packet.file_name);
    }
}

fn assert_sdp_has_valid_ssrc(packet: &ReferencePacket, header: &str, body: &[u8]) {
    assert!(
        body.starts_with(b"v=0\r\n") && body.windows(4).any(|part| part == b"\r\nm="),
        "{} SDP body is incomplete",
        packet.file_name
    );
    let body_text = std::str::from_utf8(body).expect("reference SDP is UTF-8");
    let ssrc = body_text
        .lines()
        .find_map(|line| line.strip_prefix("y="))
        .unwrap_or_else(|| panic!("{} SDP missing y= SSRC", packet.file_name));
    assert!(
        ssrc.len() == 10 && ssrc.bytes().all(|byte| byte.is_ascii_digit()),
        "{} SDP y= must be a 10-digit SSRC",
        packet.file_name
    );
    if packet.expected_status.is_none() && packet.sip_method == "INVITE" {
        let subject = header_value(header, "Subject")
            .unwrap_or_else(|| panic!("{} missing Subject", packet.file_name));
        let Some((source, target)) = subject.split_once(',') else {
            panic!("{} Subject missing two legs", packet.file_name);
        };
        assert!(
            source.ends_with(&format!(":{ssrc}")) && target.ends_with(&format!(":{ssrc}")),
            "{} Subject SSRC must match SDP y=",
            packet.file_name
        );
    }
}

fn reference_manifest(packets: &[ReferencePacket]) -> String {
    let mut method_counts = BTreeMap::<String, usize>::new();
    let mut response_count = 0usize;
    let mut sdp_packet_count = 0usize;
    let mut sdp_y_count = 0usize;
    for packet in packets {
        if packet.expected_status.is_some() {
            response_count += 1;
        } else {
            *method_counts.entry(packet.sip_method.clone()).or_default() += 1;
        }
        if packet
            .content_type
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("sdp")
        {
            sdp_packet_count += 1;
            let (_, body) = reference_body(&packet.bytes);
            sdp_y_count += std::str::from_utf8(body)
                .expect("reference SDP is UTF-8")
                .lines()
                .filter(|line| line.starts_with("y="))
                .count();
        }
    }

    let mut output = String::from(
        "version: 1\n\
source: standardized-reference\n\
source_file: gbt28181-2016-2022-baseline.md\n\
source_sha256: ",
    );
    output.push_str(REFERENCE_SOURCE_SHA256);
    output.push('\n');
    output.push_str(
        "derived_from: user-provided-gbt28181-2016-2022-sip-baseline\n\
generated_by: session/tests/sip_corpus.rs\n\
integrity_test: reference_sip_baseline_is_current_and_complete\n\
sanitization: standardized-closed-field-test-baseline\n\
quality:\n",
    );
    let tc_count = packets
        .iter()
        .map(|packet| packet.tc_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    writeln!(output, "  tc_count: {tc_count}").expect("write reference manifest");
    writeln!(output, "  packet_count: {}", packets.len()).expect("write reference manifest");
    writeln!(output, "  response_count: {response_count}").expect("write reference manifest");
    writeln!(output, "  sdp_packet_count: {sdp_packet_count}").expect("write reference manifest");
    writeln!(output, "  sdp_y_count: {sdp_y_count}").expect("write reference manifest");
    output.push_str("method_counts:\n");
    for method in [
        "REGISTER",
        "MESSAGE",
        "INVITE",
        "ACK",
        "BYE",
        "INFO",
        "SUBSCRIBE",
        "NOTIFY",
    ] {
        let count = method_counts.get(method).copied().unwrap_or_default();
        writeln!(output, "  {method}: {count}").expect("write reference manifest");
    }
    output.push_str("packets:\n");
    for packet in packets {
        let status = packet
            .expected_status
            .map_or_else(|| "null".to_owned(), |value| value.to_string());
        let content_type = packet.content_type.as_deref().unwrap_or("null");
        writeln!(output, "  - tc_id: {}", packet.tc_id).expect("write reference manifest");
        writeln!(output, "    title: {:?}", packet.tc_title).expect("write reference manifest");
        writeln!(output, "    file: extracted/{}", packet.file_name)
            .expect("write reference manifest");
        writeln!(output, "    direction: {}", packet.direction).expect("write reference manifest");
        writeln!(output, "    transport: udp").expect("write reference manifest");
        writeln!(output, "    sip_method: {}", packet.sip_method)
            .expect("write reference manifest");
        writeln!(output, "    expected_status: {status}").expect("write reference manifest");
        writeln!(output, "    content_type: {:?}", content_type).expect("write reference manifest");
        writeln!(output, "    call_id: {:?}", packet.call_id).expect("write reference manifest");
        writeln!(output, "    cseq: {:?}", packet.cseq).expect("write reference manifest");
        writeln!(output, "    sha256: {}", hex(&sha256(&packet.bytes)))
            .expect("write reference manifest");
    }
    output
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

#[test]
fn reference_sip_baseline_is_current_and_complete() {
    let dir = reference_dir();
    let source_path = dir.join("gbt28181-2016-2022-baseline.md");
    let source = fs::read_to_string(&source_path).expect("read reference SIP baseline");
    assert_eq!(
        hex(&sha256(source.as_bytes())),
        REFERENCE_SOURCE_SHA256,
        "reference baseline source hash changed"
    );

    let packets = markdown_reference_packets(&source);
    assert_eq!(
        packets.len(),
        REFERENCE_PACKET_COUNT,
        "reference packet count drifted"
    );
    let tc_count = packets
        .iter()
        .map(|packet| packet.tc_id.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    assert_eq!(tc_count, REFERENCE_TC_COUNT, "reference TC count drifted");
    for packet in &packets {
        assert_reference_packet(packet);
    }

    let mut method_counts = BTreeMap::<String, usize>::new();
    let mut response_count = 0usize;
    let mut sdp_packet_count = 0usize;
    let mut sdp_y_count = 0usize;
    for packet in &packets {
        if packet.expected_status.is_some() {
            response_count += 1;
        } else {
            *method_counts.entry(packet.sip_method.clone()).or_default() += 1;
        }
        if packet
            .content_type
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains("sdp")
        {
            sdp_packet_count += 1;
            let (_, body) = reference_body(&packet.bytes);
            sdp_y_count += std::str::from_utf8(body)
                .expect("reference SDP is UTF-8")
                .lines()
                .filter(|line| line.starts_with("y="))
                .count();
        }
    }
    assert_eq!(response_count, 53, "reference response count drifted");
    assert_eq!(method_counts.get("REGISTER").copied(), Some(4));
    assert_eq!(method_counts.get("MESSAGE").copied(), Some(31));
    assert_eq!(method_counts.get("INVITE").copied(), Some(5));
    assert_eq!(method_counts.get("ACK").copied(), Some(5));
    assert_eq!(method_counts.get("BYE").copied(), Some(5));
    assert_eq!(method_counts.get("INFO").copied(), Some(2));
    assert_eq!(method_counts.get("SUBSCRIBE").copied(), Some(2));
    assert_eq!(method_counts.get("NOTIFY").copied(), Some(1));
    assert_eq!(
        sdp_packet_count, REFERENCE_SDP_PACKET_COUNT,
        "reference SDP packet count drifted"
    );
    assert_eq!(
        sdp_y_count, REFERENCE_SDP_Y_COUNT,
        "reference SDP y= SSRC count drifted"
    );
    assert!(
        source.contains("媒体服务器 ID | `34020000002020000001`"),
        "reference baseline lost media server ID"
    );
    assert!(
        source.contains(REFERENCE_PLATFORM_ADDR),
        "reference baseline lost platform address"
    );

    let manifest = reference_manifest(&packets);
    let extracted_dir = dir.join("extracted");
    if std::env::var_os("GMV_UPDATE_SIP_REFERENCE").is_some() {
        fs::create_dir_all(&extracted_dir).expect("create reference extraction directory");
        for packet in &packets {
            fs::write(extracted_dir.join(&packet.file_name), &packet.bytes)
                .expect("write extracted reference SIP packet");
        }
        fs::write(dir.join("manifest.yaml"), &manifest).expect("write reference manifest");
    }

    let actual_manifest =
        fs::read_to_string(dir.join("manifest.yaml")).expect("read reference manifest");
    assert_eq!(
        actual_manifest, manifest,
        "reference manifest.yaml is stale"
    );
    for packet in &packets {
        let actual = fs::read(extracted_dir.join(&packet.file_name))
            .unwrap_or_else(|error| panic!("read extracted {}: {error}", packet.file_name));
        assert_eq!(actual, packet.bytes, "{} is stale", packet.file_name);
    }
    let mut expected_files = packets
        .iter()
        .map(|packet| packet.file_name.clone())
        .collect::<BTreeSet<_>>();
    let actual_files = fs::read_dir(&extracted_dir)
        .expect("read reference extraction directory")
        .map(|entry| {
            entry
                .expect("read reference extraction entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<BTreeSet<_>>();
    assert_eq!(
        actual_files, expected_files,
        "reference extraction contains stale or missing files"
    );
    expected_files.clear();
}
