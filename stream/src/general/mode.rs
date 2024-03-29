use common::err::{GlobalError, GlobalResult};
use common::log::debug;
use common::yaml_rust::Yaml;
use constructor::Get;

#[derive(Debug, Get)]
pub struct Stream {
    port: u16,
    timeout: u32,
    hook: Option<Hook>,
}

impl Stream {
    pub fn build(cfg: &Yaml) -> Self {
        if cfg.is_badvalue() || cfg["stream"].is_badvalue() {
            Stream {
                port: 23344,
                timeout: 30000,
                hook: None,
            }
        } else {
            let mut stream = &cfg["stream"];
            Stream {
                port: stream["port"].as_i64().unwrap_or(8080) as u16,
                timeout: stream["timeout"].as_i64().unwrap_or(30000) as u32,
                hook: Hook::build(stream),
            }
        }
    }
}

#[derive(Debug)]
pub struct Hook {
    on_publish: Option<String>,
    off_publish: Option<String>,
    on_play: Option<String>,
    off_play: Option<String>,
}

impl Hook {
    fn build(cfg: &Yaml) -> Option<Self> {
        if cfg["hook"].is_badvalue() {
            None
        } else {
            Some(Self {
                on_publish: cfg["hook"]["on_publish"].as_str().map(|str| str.to_string()),
                off_publish: cfg["hook"]["off_publish"].as_str().map(|str| str.to_string()),
                on_play: cfg["hook"]["on_play"].as_str().map(|str| str.to_string()),
                off_play: cfg["hook"]["off_play"].as_str().map(|str| str.to_string()),
            })
        }
    }
}


pub const AV_IO_CTX_BUFFER_SIZE: u16 = 2048;

#[derive(Debug)]
pub enum Media {
    ///video
    PS,
    MPEG4,
    H264,
    SVAC_V,
    H265,
    ///AUDIO
    G711,
    SVAC_A,
    G723_1,
    G729,
    G722_1,
    AAC,
}

impl Media {
    pub fn gb_check(tp: u8) -> GlobalResult<Self> {
        match tp {
            ///video
            //ps
            96 => { Ok(Self::PS) }
            //mpeg-4
            97 => { Ok(Self::MPEG4) }
            //h264
            98 => { Ok(Self::H264) }
            //svac
            99 => { Ok(Self::SVAC_V) }
            //h265
            100 => { Ok(Self::H265) }
            ///audio
            //g711
            8 => { Ok(Self::G711) }
            //svac
            20 => { Ok(Self::SVAC_A) }
            //g723-1
            4 => { Ok(Self::G723_1) }
            //g729
            18 => { Ok(Self::G729) }
            //g722.1
            9 => { Ok(Self::G722_1) }
            //aac
            102 => { Ok(Self::AAC) }
            _ => {
                Err(GlobalError::new_biz_error(4004, &*format!("rtp type = {tp},GB28181未定义类型。"), |msg| debug!("{msg}")))
            }
        }
    }

    pub fn impl_check(tp: u8) -> GlobalResult<Self> {
        match tp {
            ///video
            //ps
            96 => { Ok(Self::PS) }
            //h264
            98 => { Ok(Self::H264) }
            _ => {
                Self::gb_check(tp)
                    .and_then(|v|
                        Err(GlobalError::new_biz_error(4005, &*format!("rtp type = {:?},系统暂不支持。", v), |msg| debug!("{msg}"))))
            }
        }
    }
}


#[cfg(test)]
mod tests {
    use crate::general::mode::Stream;

    #[test]
    pub fn test_build_stream() {
        let binding = common::get_config();
        let cfg = binding.get(0).unwrap();
        println!("{:?}", Stream::build(cfg));
    }
}