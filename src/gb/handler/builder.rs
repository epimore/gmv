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
use crate::gb::handler::parser;

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

#[cfg(test)]
mod tests {
    use common::chrono::Local;

    #[test]
    fn test_date_format() {
        let now = Local::now();
        let date = now.format("%Y-%m-%dT%H:%M:%S").to_string();
        println!("{date}");
    }
}