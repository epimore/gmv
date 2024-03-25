pub mod header {
    use rsip::headers::Via;
    use rsip::prelude::{HasHeaders, HeadersExt};
    use rsip::{Header, Request, Response};
    use common::anyhow::anyhow;
    use common::err::{GlobalResult, TransError};
    use common::err::GlobalError::{SysErr};
    use common::log::{warn};

    pub fn get_device_id_by_request(req: &Request) -> GlobalResult<String> {
        let from_user = req.from_header()
            .hand_err(|msg| warn!("{msg}"))?
            .uri().hand_err(|msg| warn!("{msg}"))?
            .auth.ok_or(SysErr(anyhow!("user is none")))
            .hand_err(|msg| warn!("{msg}"))?
            .user;
        Ok(from_user)
    }

    pub fn get_device_id_by_response(req: &Response) -> GlobalResult<String> {
        let from_user = req.to_header()
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

    pub fn get_domain(req: &Request) -> GlobalResult<String> {
        let to_uri = req.to_header().hand_err(|msg| warn!("{msg}"))?.uri().hand_err(|msg| warn!("{msg}"))?;
        Ok(to_uri.host_with_port.to_string())
    }

    pub fn get_gb_version(req: &Request) -> Option<String> {
        for header in req.headers().iter() {
            match header {
                Header::Other(key, val) => {
                    if key.eq("X-GB-Ver") {
                        return match &val[..] {
                            "1.0" => Some("GB/T 28181—2011".to_string()),
                            "1.1" => Some("GB/T 28181—2011-1".to_string()),
                            "2.0" => Some("GB/T 28181—2016".to_string()),
                            "3.0" => Some("GB/T 28181—2022".to_string()),
                            &_ => Some(val.to_string())
                        };
                    }
                }
                _ => { continue; }
            };
        }
        None
    }
}