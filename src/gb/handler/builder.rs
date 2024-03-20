use std::net::SocketAddr;
use rsip::{Error, header, Header, headers, Method, Param, param, Request, SipMessage, uri, Uri};
use rsip::Header::Via;
use rsip::headers::{Date, header, typed};
use rsip::message::HeadersExt;
use rsip::Param::{Other};
use rsip::param::{OtherParam, OtherParamValue};
use rsip::prelude::*;
use common::chrono::Local;
use common::err::{GlobalResult, TransError};
use common::log::warn;
use common::rand;
use common::rand::Rng;
use uuid::Uuid;
use common::anyhow::anyhow;
use common::chrono::format::InternalFixed;
use common::err::GlobalError::SysErr;
use common::net::shard::Bill;
use crate::common::SessionConf;
use crate::gb::handler::parser;
use crate::gb::shard::rw::RWSession;
use crate::storage::entity::{GmvDevice, GmvOauth};
use crate::storage::mapper;

pub fn build_401_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
    let mut response_header = build_response_header(req, socket_addr)?;
    let other_header = Header::Other(String::from("X-GB-Ver"), String::from("2.0"));
    response_header.push(other_header);
    Ok(rsip::Response {
        status_code: 401.into(),
        headers: response_header,
        version: rsip::Version::V2,
        body: Default::default(),
    }.into())
}

pub fn unauthorized_register_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
    let mut response_header = build_response_header(req, socket_addr)?;
    let other_header = Header::Other(String::from("X-GB-Ver"), String::from("2.0"));
    response_header.push(other_header);
    let domain = parser::header::get_domain(&req)?;
    response_header.push(typed::WwwAuthenticate {
        realm: domain,
        algorithm: Some(headers::auth::Algorithm::Md5),
        qop: Some(headers::auth::Qop::Auth),
        nonce: Uuid::new_v4().as_simple().to_string(),
        ..Default::default()
    }.into());
    Ok(rsip::Response {
        status_code: 401.into(),
        headers: response_header,
        version: rsip::Version::V2,
        body: Default::default(),
    }.into())
}

pub fn build_register_ok_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
    let mut response_header = build_response_header(req, socket_addr)?;
    let other_header = Header::Other(String::from("X-GB-Ver"), String::from("2.0"));
    response_header.push(other_header);
    Ok(rsip::Response {
        status_code: 200.into(),
        headers: response_header,
        version: rsip::Version::V2,
        body: Default::default(),
    }.into())
}

pub fn build_logout_ok_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
    let mut response_header = build_response_header(req, socket_addr)?;
    let now = Local::now();
    let date = now.format("%Y-%m-%dT%H:%M:%S").to_string();
    let date_header = rsip::headers::date::Date::new(date);
    response_header.push(date_header.into());
    Ok(rsip::Response {
        status_code: 200.into(),
        headers: response_header,
        version: rsip::Version::V2,
        body: Default::default(),
    }.into())
}


fn build_response_header(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<rsip::Headers> {
    let via_header = parser::header::get_via_header(req)?;
    let mut params = via_header.params().hand_err(|msg| warn!("{msg}"))?;
    if params.iter_mut().any(|param| {
        if (*param).eq(&Other(OtherParam::from("rport"), None)) {
            *param = Other(OtherParam::from("rport"), Some(OtherParamValue::from(socket_addr.port().to_string())));
            return true;
        }
        false
    }) {
        params.push(Param::Received(param::Received::from(socket_addr.ip().to_string())));
    }
    let mut via: typed::Via = via_header.typed().hand_err(|msg| warn!("{msg}"))?;
    via.params = params;
    let mut headers: rsip::Headers = Default::default();
    headers.push(Via(via.untyped()));
    headers.push(req.from_header().hand_err(|msg| warn!("{msg}"))?.clone().into());
    let mut rng = rand::thread_rng();
    let to = req.to_header().hand_err(|msg| warn!("{msg}"))?.typed().hand_err(|msg| warn!("{msg}"))?;
    let to = to.with_tag(rng.gen_range(123456789u32..987654321u32).to_string().into());
    headers.push(to.into());
    headers.push(req.call_id_header().hand_err(|msg| warn!("{msg}"))?.clone().into());
    headers.push(req.cseq_header().hand_err(|msg| warn!("{msg}"))?.clone().into());
    headers.push(Header::ContentLength(Default::default()));
    headers.push(rsip::headers::UserAgent::new("GMV 0.1").into());
    req.contact_header().map(|contact| headers.push(contact.clone().into())).hand_err(|msg| warn!("{msg}"))?;
    Ok(headers)
}

/// 构建下发请求
fn build_request_header(channel_id: Option<&String>, device_id: &String, expires: bool, contact: bool, from_tag: Option<&str>, to_tag: Option<&str>)
                        -> GlobalResult<(rsip::Headers, Uri)> {
    let bill = RWSession::get_bill_by_device_id(device_id)?.ok_or(SysErr(anyhow!("设备：{device_id}，未注册或已离线"))).hand_err(|msg| warn!("{msg}"))?;
    let mut dst_id = device_id;
    if channel_id.is_some() {
        let channel_id = channel_id.unwrap();
        let channel_status = mapper::get_device_channel_status(device_id, channel_id)?.ok_or(SysErr(anyhow!("设备：{device_id} - 通道：{channel_id}，未知或无效")))?;
        match &channel_status.to_ascii_uppercase()[..] {
            "ON" | "ONLINE" => { dst_id = channel_id }
            _ => {
                anyhow!("设备：{device_id} - 通道：{channel_id}，已下线");
            }
        }
    }
    let socket_addr = bill.get_from().to_string();
    let conf = SessionConf::get_session_conf();
    let server_ip = &conf.get_wan_ip().to_string();
    let server_port = conf.get_wan_port();
    //domain宜采用ID统一编码的前十位编码,扩展支持十位编码加“.spvmn.cn”后缀格式,或采用IP:port格式,port宜采用5060;这里统一使用device_id的前十位,不再调用DB进行判断原设备的使用方式
    // let gmv_device = GmvDevice::query_gmv_device_by_device_id(&device_id.to_string())?.ok_or(SysErr(anyhow!("设备：{device_id}-{channel_id:?}未注册或已离线。")))?;
    let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id)?
        .ok_or(SysErr(anyhow!("device id = [{}] 未知设备",device_id)))
        .hand_err(|msg| warn!("{msg}"))?;
    if oauth.get_status() == &0u8 {
        warn!("device id = [{}] 未启用设备,无法下发指令",&device_id);
    }
    let domain_id = oauth.get_domain_id();
    let domain = oauth.get_domain();

    let transport = bill.get_protocol().get_value();
    let uri_str = format!("sip:{}@{}", dst_id, domain);
    let uri = uri::Uri::try_from(uri_str).hand_err(|msg| warn!("{msg}"))?;
    let mut rng = rand::thread_rng();
    let mut headers: rsip::Headers = Default::default();
    headers.push(rsip::headers::Via::new(format!("SIP/2.0/{} {}:{};rport;branch=z9hG4bK{}", transport, server_ip, server_port, rng.gen_range(123456789u32..987654321u32))).into());
    headers.push(rsip::headers::From::new(format!("<sip:{}@{}>;tag={}", domain_id, domain, from_tag.unwrap_or(&rng.gen_range(123456789u32..987654321u32).to_string()))).into());
    to_tag.map(|tag| {
        headers.push(rsip::headers::To::new(format!("<sip:{}@{}>;tag={}", dst_id, domain, tag)).into())
    }).unwrap_or_else(
        || { headers.push(rsip::headers::To::new(format!("<sip:{}@{}>", dst_id, domain)).into()) }
    );
    headers.push(rsip::headers::CallId::new(Uuid::new_v4().as_simple().to_string()).into());
    if expires {
        let expires = RWSession::get_expires_by_device_id(device_id)?.ok_or(SysErr(anyhow!("device id = [{}] 未知设备",device_id)))?;
        headers.push(rsip::headers::Expires::new(expires.as_secs().to_string()).into());
    }
    if contact {
        headers.push(rsip::headers::Contact::new(format!("<sip:{}@{}:{}>", domain_id, server_ip, server_port)).into());
    }
    headers.push(rsip::headers::MaxForwards::new("70").into());
    headers.push(rsip::headers::UserAgent::new("GMV 0.1").into());
    Ok((headers, uri))
}

#[cfg(test)]
mod tests {
    use common::chrono::Local;

    #[test]
    fn test_date_format() {
        let now = Local::now();
        let date = now.format("%Y-%m-%dT%H:%M:%S").to_string();
        println!("{date}");
    }

    #[test]
    fn test_region() {
        let device_id = &String::from("34020000001111131043");
        assert_eq!("3402000000", &device_id[0..10]);
    }
}