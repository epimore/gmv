use base::bytes::{Buf, Bytes, BytesMut};
use std::str;

/// 粘包拆包处理（标准 + 容错）
pub fn complete_pkt(buffer: &mut BytesMut) -> Option<Vec<Bytes>> {
    let mut packets = Vec::new();

    loop {
        // 1️⃣ 查找头部结束符
        let Some(header_end) = find_header_end(buffer) else {
            // header 未收完整
            break;
        };

        // 2️⃣ 提取 header
        let headers = &buffer[..header_end];
        let headers_str = match str::from_utf8(headers) {
            Ok(v) => v,
            Err(_) => {
                // 非 UTF8 数据，丢弃缓冲，防止死循环
                buffer.clear();
                break;
            }
        };

        // 3️⃣ 解析 Content-Length
        let content_length = parse_content_length(headers_str).unwrap_or(0);
        let total_len = header_end + 4 + content_length;

        if buffer.len() < total_len {
            // 未收全，退出等待下次
            break;
        }

        // 4️⃣ 提取完整报文
        let pkt = buffer.split_to(total_len).freeze();
        packets.push(pkt);

        // 5️⃣ 清理多余空行
        while buffer.starts_with(b"\r\n") {
            buffer.advance(2);
        }

        // 6️⃣ 检查下一条报文是否存在
        // 如果后续不是以 SIP/2.0 或 方法名开头，则继续等待
        if !maybe_next_packet(buffer) {
            break;
        }
    }

    if packets.is_empty() {
        None
    } else {
        Some(packets)
    }
}

/// 查找 SIP 头部结束标志 "\r\n\r\n"
fn find_header_end(buf: &BytesMut) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 解析 Content-Length (大小写不敏感 + 兼容简写 l:)
fn parse_content_length(headers: &str) -> Option<usize> {
    for line in headers.lines() {
        let line_lower = line.trim().to_ascii_lowercase();
        if let Some(v) = line_lower.strip_prefix("content-length:") {
            return v.trim().parse::<usize>().ok();
        }
        if let Some(v) = line_lower.strip_prefix("l:") {
            return v.trim().parse::<usize>().ok();
        }
    }
    None
}

/// 判断当前缓冲区是否可能是下一条 SIP 报文的开头
fn maybe_next_packet(buf: &BytesMut) -> bool {
    if buf.len() < 8 {
        return false;
    }

    // 直接字节级检查，避免UTF-8转换开销
    let prefix = &buf[..std::cmp::min(24, buf.len())];

    // 检查SIP响应如: "SIP/2.0 200 "
    if prefix.len() >= 12 && &prefix[..8] == b"SIP/2.0 " {
        // 检查状态码是3位数字加空格: "200 "
        let status_part = &prefix[8..12];
        return status_part[0].is_ascii_digit() &&
            status_part[1].is_ascii_digit() &&
            status_part[2].is_ascii_digit() &&
            status_part[3] == b' ';
    }

    // 检查SIP请求如："REGISTER sip:130909115229300920@10.64.49.44:7100 SIP/2.0"
    const METHOD_PREFIXES: &[&[u8]] = &[
        b"REGISTER ", b"INVITE ", b"ACK ", b"BYE ", b"CANCEL ",
        b"OPTIONS ", b"MESSAGE ", b"INFO ", b"PRACK ", b"SUBSCRIBE ",
        b"NOTIFY ", b"UPDATE ", b"REFER ", b"PUBLISH ",
    ];

    for method_prefix in METHOD_PREFIXES {
        if prefix.len() >= method_prefix.len() + 4 &&
            prefix.starts_with(method_prefix) {

            // 检查 "sip:" 或 "sips"
            let uri_part = &prefix[method_prefix.len()..];
            return uri_part.starts_with(b"sip:") || uri_part.starts_with(b"sips:");
        }
    }

    false
}
