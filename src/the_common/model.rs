use common::anyhow::anyhow;
use common::err::GlobalError::SysErr;
use common::err::GlobalResult;

pub enum StreamMode {
    Udp,
    TcpActive,
    TcpPassive,
}

impl StreamMode {
    pub fn build(m: u8) -> GlobalResult<Self> {
        match m {
            0 => { Ok(StreamMode::Udp) }
            1 => { Ok(StreamMode::TcpActive) }
            2 => { Ok(StreamMode::TcpPassive) }
            _ => { Err(SysErr(anyhow!("无效流模式"))) }
        }
    }
}