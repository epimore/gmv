pub mod header {
    use rsip::headers::Via;
    use rsip::prelude::{HasHeaders, HeadersExt};
    use rsip::{Header, Request};
    use common::anyhow::anyhow;
    use common::err::{GlobalError, GlobalResult, TransError};
    use common::err::GlobalError::{BizErr, SysErr};
    use common::log::{warn};

    pub fn get_device_id(req: &Request) -> GlobalResult<String> {
        let from_user = req.from_header()
            .hand_err(|msg| warn!("{msg}"))?
            .uri().hand_err(|msg| warn!("{msg}"))?
            .auth.ok_or(SysErr(anyhow!("user is none")))
            .hand_err(|msg| warn!("{msg}"))?
            .user;
        Ok(from_user)
    }

    pub fn get_via_header(req: &Request) -> GlobalResult<&Via> {
        let via = req.via_header().hand_err(|msg| warn!("{msg}"))?;
        Ok(via)
    }

    pub fn get_transport(req: &Request) -> GlobalResult<String> {
        let transport = get_via_header(req)?.trasnport().hand_err(|msg| warn!("{msg}"))?.to_string();
        Ok(transport)
    }

    pub fn get_local_addr(req: &Request) -> GlobalResult<String> {
        let local_addr = get_via_header(req)?.uri().hand_err(|msg| warn!("{msg}"))?.host_with_port.to_string();
        Ok(local_addr)
    }

    pub fn get_from(req: &Request) -> GlobalResult<String> {
        let from = req.from_header().hand_err(|msg| warn!("{msg}"))?.uri().hand_err(|msg| warn!("{msg}"))?.to_string();
        Ok(from)
    }

    pub fn get_to(req: &Request) -> GlobalResult<String> {
        let to = req.to_header().hand_err(|msg| warn!("{msg}"))?.uri().hand_err(|msg| warn!("{msg}"))?.to_string();
        Ok(to)
    }

    pub fn get_expires(req: &Request) -> GlobalResult<u32> {
        let expires = req.expires_header()
            .ok_or(SysErr(anyhow!("无参数expires")))
            .hand_err(|msg| warn!("{msg}"))?
            .seconds().hand_err(|msg| warn!("{msg}"))?;
        Ok(expires)
    }

    pub fn get_gb_version(req: &Request) -> Option<String> {
        let other_1_0 = Header::Other("X-GB-Ver".into(), "1.0".into());
        let other_1_1 = Header::Other("X-GB-Ver".into(), "1.1".into());
        let other_2_0 = Header::Other("X-GB-Ver".into(), "2.0".into());
        let other_3_0 = Header::Other("X-GB-Ver".into(), "3.0".into());
        let x_gb_ver = String::from("X-GB-Ver");
        for header in req.headers().iter() {
            match header {
                other_1_0 => {
                    return Some("GB/T 28181—2011".to_string());
                }
                other_1_1 => {
                    return Some("GB/T 28181—2011-1".to_string());
                }
                other_2_0 => {
                    return Some("GB/T 28181—2016".to_string());
                }
                other_3_0 => {
                    return Some("GB/T 28181—2022".to_string());
                }
                Header::Other(x_gb_ver, val) => {
                    return Some(val.to_string());
                }
                _ => { continue; }
            }
        }
        None
    }
}