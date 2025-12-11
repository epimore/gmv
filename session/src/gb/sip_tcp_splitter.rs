use std::collections::VecDeque;
use base::bytes::{Buf, Bytes, BytesMut};
use std::str;

/// SIP over TCP 拆包器：
/// - 支持粘包、拆包、多包连续
/// - 支持无 Content-Length（国产设备常见）
/// - 不使用 UTF-8 解析（SIP header 是 ASCII）
/// - 容错 CL 格式
/// - 处理非法数据（防御性编码）
pub fn complete_pkt(buffer: &mut BytesMut, packets: &mut VecDeque<Bytes>) {
    loop {
        // 必须先能找到 header 结束符
        let Some(header_end) = find_header_end(buffer) else {
            break;
        };

        let header_bytes = &buffer[..header_end + 4]; // 包含 \r\n\r\n
        let headers = match std::str::from_utf8(header_bytes) {
            Ok(v) => v,
            Err(_) => {
                // SIP header 必须是 ASCII，解析失败表示脏数据
                // 清掉无效数据，但不清空整个 buffer，尽量避免丢包
                buffer.advance(header_end + 4);
                continue;
            }
        };

        // 解析 Content-Length
        let content_length = parse_content_length(headers);

        // 总长度 = header + body
        let total_len = header_end + 4 + content_length;

        if buffer.len() < total_len {
            // 半包 body，等待更多数据
            break;
        }

        // 提取完整报文
        let pkt = buffer.split_to(total_len).freeze();
        packets.push_back(pkt);

        // 清除额外空行（部分设备会在包之间增加空行）
        while buffer.starts_with(b"\r\n") {
            buffer.advance(2);
        }

        // 检查下一条是否有可能是 SIP 包
        if !maybe_sip_start(buffer) {
            break;
        }
    }
}

/// 查找 \r\n\r\n 的位置
fn find_header_end(buf: &BytesMut) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// 解析 Content-Length（支持大小写、前后空格）
/// 若未找到，则返回 0（国产设备常见）
fn parse_content_length(headers: &str) -> usize {
    for line in headers.lines() {
        let line_lower = line.to_ascii_lowercase();
        if line_lower.starts_with("content-length:") {
            // 截取 ":" 后部分
            if let Some(v) = line.splitn(2, ':').nth(1) {
                return v.trim().parse::<usize>().unwrap_or(0);
            }
        }
    }
    0
}

/// 粗略判断下一条报文是否可能以 SIP 开始
/// 只用于避免误判，不能严格验证
fn maybe_sip_start(buf: &BytesMut) -> bool {
    if buf.len() < 4 {
        return false;
    }

    // SIP/2.0 Response
    if buf.starts_with(b"SIP/2.0 ") {
        return true;
    }

    // Request 行：METHOD SP URI
    // METHOD：REGISTER / INVITE / MESSAGE / OPTIONS / BYE / ACK / CANCEL / INFO 等
    const METHODS: [&[u8]; 9] = [
        b"REGISTER", b"INVITE", b"MESSAGE", b"OPTIONS",
        b"BYE", b"ACK", b"CANCEL", b"INFO", b"UPDATE",
    ];

    for m in METHODS {
        if buf.starts_with(m) {
            return true;
        }
    }

    false
}
