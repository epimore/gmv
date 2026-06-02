use base::bytes::{Buf, Bytes, BytesMut};
use base::log::warn;
use std::collections::VecDeque;

const MAX_TCP_SIP_BUFFER_LEN: usize = 1024 * 1024;

enum ContentLength {
    Present(usize),
    Missing,
    Invalid,
}

pub fn complete_pkt(buffer: &mut BytesMut, packets: &mut VecDeque<Bytes>) {
    loop {
        let Some(header_end) = find_header_end(buffer) else {
            if buffer.len() > MAX_TCP_SIP_BUFFER_LEN {
                warn!("SIP TCP buffer exceeds {MAX_TCP_SIP_BUFFER_LEN} bytes without header end");
                discard_leading_garbage(buffer);
            }
            break;
        };

        let header_bytes = &buffer[..header_end + 4];
        let headers = match std::str::from_utf8(header_bytes) {
            Ok(v) => v,
            Err(_) => {
                warn!("drop SIP TCP packet with non-ASCII header");
                discard_invalid_message(buffer, header_end + 4);
                continue;
            }
        };

        let content_length = match parse_content_length(headers) {
            ContentLength::Present(content_length) => content_length,
            ContentLength::Missing if can_omit_content_length(headers) => 0,
            ContentLength::Missing => {
                warn!("drop SIP TCP packet without Content-Length");
                discard_invalid_message(buffer, header_end + 4);
                continue;
            }
            ContentLength::Invalid => {
                warn!("drop SIP TCP packet with invalid Content-Length");
                discard_invalid_message(buffer, header_end + 4);
                continue;
            }
        };

        let total_len = header_end + 4 + content_length;
        if buffer.len() < total_len {
            break;
        }

        packets.push_back(buffer.split_to(total_len).freeze());

        while buffer.starts_with(b"\r\n") {
            buffer.advance(2);
        }

        if !maybe_sip_start(buffer) {
            discard_leading_garbage(buffer);
        }
        if buffer.is_empty() || !maybe_sip_start(buffer) {
            break;
        }
    }
}

fn find_header_end(buf: &BytesMut) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length(headers: &str) -> ContentLength {
    for line in headers.lines() {
        let line_lower = line.to_ascii_lowercase();
        if line_lower.starts_with("content-length:") || line_lower.starts_with("l:") {
            if let Some(v) = line.splitn(2, ':').nth(1) {
                return v
                    .trim()
                    .parse::<usize>()
                    .map(ContentLength::Present)
                    .unwrap_or(ContentLength::Invalid);
            }
            return ContentLength::Invalid;
        }
    }
    ContentLength::Missing
}

fn can_omit_content_length(headers: &str) -> bool {
    if has_content_type(headers) {
        return false;
    }
    match first_token(headers) {
        Some("MESSAGE" | "NOTIFY" | "INVITE" | "INFO" | "SUBSCRIBE" | "PUBLISH") => false,
        _ => true,
    }
}

fn has_content_type(headers: &str) -> bool {
    headers.lines().any(|line| {
        let line_lower = line.to_ascii_lowercase();
        line_lower.starts_with("content-type:") || line_lower.starts_with("c:")
    })
}

fn first_token(headers: &str) -> Option<&str> {
    headers.lines().next()?.split_whitespace().next()
}

fn discard_invalid_message(buffer: &mut BytesMut, search_start: usize) {
    if search_start >= buffer.len() {
        buffer.clear();
        return;
    }
    if let Some(pos) = find_sip_start(&buffer[search_start..]) {
        buffer.advance(search_start + pos);
    } else {
        buffer.clear();
    }
}

fn discard_leading_garbage(buffer: &mut BytesMut) {
    if buffer.is_empty() || maybe_sip_start(buffer) {
        return;
    }
    if let Some(pos) = find_sip_start(buffer) {
        buffer.advance(pos);
    } else {
        buffer.clear();
    }
}

fn find_sip_start(buf: &[u8]) -> Option<usize> {
    (0..buf.len()).find(|&idx| maybe_sip_start(&buf[idx..]))
}

fn maybe_sip_start(buf: &[u8]) -> bool {
    if buf.len() < 4 {
        return false;
    }

    if buf.starts_with(b"SIP/2.0 ") {
        return true;
    }

    const METHODS: [&[u8]; 14] = [
        b"REGISTER",
        b"INVITE",
        b"MESSAGE",
        b"NOTIFY",
        b"OPTIONS",
        b"BYE",
        b"ACK",
        b"CANCEL",
        b"INFO",
        b"SUBSCRIBE",
        b"PRACK",
        b"PUBLISH",
        b"REFER",
        b"UPDATE",
    ];

    METHODS.iter().any(|method| buf.starts_with(method))
}
