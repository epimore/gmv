use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::net::SocketAddr;
use common::bytes::{Buf, BufMut, Bytes, BytesMut};

pub struct TcpRtpBuffer {
    //AHashMap ?
    inner: HashMap<(SocketAddr, SocketAddr), BytesMut>,
}

impl TcpRtpBuffer {
    pub fn register_buffer() -> Self {
        Self { inner: Default::default() }
    }

    pub fn fresh_data(&mut self, local_addr: SocketAddr, remote_addr: SocketAddr, data: Bytes) -> Vec<Bytes> {
        match self.inner.entry((local_addr, remote_addr)) {
            Entry::Occupied(mut occ) => {
                let buffer = occ.get_mut();
                buffer.put(data);
                Self::split_data(buffer)
            }
            Entry::Vacant(vac) => {
                let mut buffer = BytesMut::with_capacity(10240);
                buffer.put(data);
                let vec = Self::split_data(&mut buffer);
                vac.insert(buffer);
                vec
            }
        }
    }
    const TCP_RTP_DATA_LEN: usize = 2;
    //tcp封装的Rtp包：2 bytes Data_len + N bytes Rtp_data(rtp_base_header_len = 12)
    const TCP_DATA_BASE_LEN: usize = 14;
    fn split_data(buffer: &mut BytesMut) -> Vec<Bytes> {
        let mut vec = Vec::new();
        loop {
            let buffer_len = buffer.len();
            if buffer_len < Self::TCP_DATA_BASE_LEN {
                break;
            }
            let split_len = buffer.get_u16() as usize + Self::TCP_RTP_DATA_LEN;
            if buffer_len < split_len {
                break;
            }
            let mut split_data = buffer.split_to(split_len);
            let rtp_data = split_data.split_off(Self::TCP_RTP_DATA_LEN).freeze();
            vec.push(rtp_data);
        }
        vec
    }

    pub fn remove_map(&mut self, local_addr: SocketAddr, remote_addr: SocketAddr) {
        self.inner.remove(&(local_addr, remote_addr));
    }
}

#[cfg(test)]
mod test {
    use std::collections::VecDeque;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use common::bytes::{Buf, BytesMut};

    #[test]
    fn test_deque_vec() {
        let mut d = VecDeque::new();
        d.push_back(1);
        d.push_back(2);
        d.push_back(3);
        d.push_back(4);
        d.push_back(5);
        d.push_back(6);
        d.push_back(7);
        d.push_back(8);
        assert_eq!(d.pop_front(), Some(1));
        d.push_back(1);
        assert_eq!(d.pop_front(), Some(2));
        d.push_back(1);
        assert_eq!(d.pop_front(), Some(3));
        d.push_back(1);
        d.insert(0, 444);
        assert_eq!(d.pop_front(), Some(444));
        d.push_back(1);
    }

    #[test]
    fn test_socket_addr_to_string() {
        let socket = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080);
        println!("{}", socket.to_string());
    }

    #[test]
    fn test_tcp_rtp_buffer() {
        let mut buffer = BytesMut::with_capacity(2048);
        buffer.extend_from_slice(&[0x00, 0x10]); // 长度字段：16
        buffer.extend_from_slice(&[0x90, 0x60, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x20, 0x00]); // RTP 数据示例
        let data_len = buffer.get_u16() as usize;
        let rtp_data = buffer.split_to(data_len).freeze();
        println!("{:02x?}", rtp_data.to_vec());
        println!("buffer len: {}, data: {:02x?}", buffer.len(), buffer.to_vec());
    }
}