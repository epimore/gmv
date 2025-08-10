use std::net::SocketAddr;
use std::fmt::Write;
use base::log::{error};
use rsip::{Error, Header, header, headers, Method, Param, param, Request, Response, SipMessage, uri, Uri};
use rsip::Header::Via;
use rsip::headers::typed;
use rsip::message::HeadersExt;
use rsip::param::{OtherParam, OtherParamValue};
use rsip::Param::Other;
use rsip::prelude::*;
use uuid::Uuid;

use anyhow::anyhow;
use base::chrono::Local;
use base::exception::{GlobalResult, GlobalResultExt};
use base::exception::GlobalError::SysErr;
use base::log::warn;
use base::rand::prelude::StdRng;
use base::rand::{Rng, SeedableRng, thread_rng};

use crate::gb::handler::parser;
use crate::gb::SessionConf;
use crate::gb::core::event::Ident;
use crate::gb::core::rw::RWSession;
use crate::storage::entity::GmvOauth;
use crate::storage::mapper;
use crate::state::model::{PtzControlModel, StreamMode};

pub struct ResponseBuilder;

impl ResponseBuilder {
    pub fn get_tag_by_header_to(response: &Response) -> GlobalResult<String> {
        let tag = response.to_header()
            .hand_log(|msg| warn!("{msg}"))?
            .tag()
            .hand_log(|msg| warn!("{msg}"))?
            .ok_or(SysErr(anyhow!("to tag is none")))?
            .to_string();
        Ok(tag)
    }
    pub fn get_tag_by_header_from(response: &Response) -> GlobalResult<String> {
        let tag = response.from_header()
            .hand_log(|msg| warn!("{msg}"))?
            .tag()
            .hand_log(|msg| warn!("{msg}"))?
            .ok_or(SysErr(anyhow!("from tag is none")))?
            .to_string();
        Ok(tag)
    }

    pub fn build_401_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
        let mut response_header = Self::build_response_header(req, socket_addr)?;
        let other_header = Header::Other(String::from("X-GB-Ver"), String::from("3.0"));
        response_header.push(other_header);
        Ok(rsip::Response {
            status_code: 401.into(),
            headers: response_header,
            version: rsip::Version::V2,
            body: Default::default(),
        }.into())
    }

    pub fn unauthorized_register_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
        let mut response_header = Self::build_response_header(req, socket_addr)?;
        let other_header = Header::Other(String::from("X-GB-Ver"), String::from("3.0"));
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
        let mut response_header = Self::build_response_header(req, socket_addr)?;
        let other_header = Header::Other(String::from("X-GB-Ver"), String::from("3.0"));
        response_header.push(other_header);
        Ok(rsip::Response {
            status_code: 200.into(),
            headers: response_header,
            version: rsip::Version::V2,
            body: Default::default(),
        }.into())
    }

    pub fn build_logout_ok_response(req: &Request, socket_addr: &SocketAddr) -> GlobalResult<SipMessage> {
        let mut response_header = Self::build_response_header(req, socket_addr)?;
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
        let mut params = via_header.params().hand_log(|msg| warn!("{msg}"))?;
        if params.iter_mut().any(|param| {
            if (*param).eq(&Other(OtherParam::from("rport"), None)) {
                *param = Other(OtherParam::from("rport"), Some(OtherParamValue::from(socket_addr.port().to_string())));
                return true;
            }
            false
        }) {
            params.push(Param::Received(param::Received::from(socket_addr.ip().to_string())));
        }
        let mut via: typed::Via = via_header.typed().hand_log(|msg| warn!("{msg}"))?;
        via.params = params;
        let mut headers: rsip::Headers = Default::default();
        headers.push(Via(via.untyped()));
        headers.push(req.from_header().hand_log(|msg| warn!("{msg}"))?.clone().into());
        let mut rng = thread_rng();
        let to = req.to_header().hand_log(|msg| warn!("{msg}"))?.typed().hand_log(|msg| warn!("{msg}"))?;
        let to = to.with_tag(rng.gen_range(123456789u32..987654321u32).to_string().into());
        headers.push(to.into());
        headers.push(req.call_id_header().hand_log(|msg| warn!("{msg}"))?.clone().into());
        headers.push(req.cseq_header().hand_log(|msg| warn!("{msg}"))?.clone().into());
        headers.push(Header::ContentLength(Default::default()));
        headers.push(rsip::headers::UserAgent::new("GMV 0.1").into());
        let _ = req.contact_header().map(|contact| headers.push(contact.clone().into()));
        Ok(headers)
    }
}

pub struct RequestBuilder;

#[allow(unused)]
impl RequestBuilder {
    pub async fn query_preset(device_id: &String, channel_id_opt: Option<&String>) -> GlobalResult<(Ident, SipMessage)> {
        let xml = XmlBuilder::query_preset(channel_id_opt.unwrap_or(device_id));
        let message_request = Self::build_message_request(channel_id_opt, device_id, xml).await;
        message_request
    }
    pub async fn query_device_info(device_id: &String) -> GlobalResult<(Ident, SipMessage)> {
        let xml = XmlBuilder::query_device_info(device_id);
        let message_request = Self::build_message_request(None, device_id, xml).await;
        message_request
    }
    pub async fn query_device_catalog(device_id: &String) -> GlobalResult<(Ident, SipMessage)> {
        let xml = XmlBuilder::query_device_catalog(device_id);
        let message_request = Self::build_message_request(None, device_id, xml).await;
        message_request
    }
    pub async fn subscribe_device_catalog(device_id: &String) -> GlobalResult<(Ident, SipMessage)> {
        let xml = XmlBuilder::query_device_catalog(device_id);
        let message_request = Self::build_subscribe_request(device_id, xml).await;
        message_request
    }

    pub async fn control_snapshot_image(device_id: &String, channel_id: &String, num: u8, interval: u8, uri: &String, session_id: &String) -> GlobalResult<(Ident, SipMessage)> {
        let xml = XmlBuilder::control_snapshot_image(channel_id, num, interval, uri, session_id);
        let message_request = Self::build_message_request(Some(channel_id), device_id, xml).await;
        message_request
    }
    pub async fn control_ptz(ptz_control_model: &PtzControlModel) -> GlobalResult<(Ident, SipMessage)> {
        let xml = XmlBuilder::control_ptz(ptz_control_model);
        let message_request = Self::build_message_request(Some(&ptz_control_model.channelId), &ptz_control_model.deviceId, xml).await;
        message_request
    }

    async fn build_message_request(channel_id_opt: Option<&String>, device_id: &String, body: String) -> GlobalResult<(Ident, SipMessage)> {
        let (mut headers, uri) = Self::build_request_header(channel_id_opt, device_id, false, false, None, None).await?;
        let call_id_str = Uuid::new_v4().as_simple().to_string();
        headers.push(rsip::headers::CallId::new(&call_id_str).into());
        let mut rng = thread_rng();
        let cs_eq_str = format!("{} MESSAGE", rng.gen_range(12u8..255u8));
        let cs_eq = rsip::headers::CSeq::new(&cs_eq_str).into();
        headers.push(cs_eq);
        headers.push(rsip::headers::ContentType::new("Application/MANSCDP+xml").into());
        headers.push(rsip::headers::ContentLength::from(body.len() as u32).into());
        let request_msg: SipMessage = Request {
            method: Method::Message,
            uri,
            headers,
            version: rsip::common::version::Version::V2,
            body: body.as_bytes().to_vec(),
        }.into();
        let ident = Ident::new(device_id.to_string(), call_id_str, cs_eq_str);
        Ok((ident, request_msg))
    }

    async fn build_subscribe_request(device_id: &String, body: String) -> GlobalResult<(Ident, SipMessage)> {
        let (mut headers, uri) = Self::build_request_header(None, device_id, true, true, None, None).await?;
        let call_id_str = Uuid::new_v4().as_simple().to_string();
        headers.push(rsip::headers::CallId::new(&call_id_str).into());
        let mut rng = thread_rng();
        let cs_eq_str = format!("{} SUBSCRIBE", rng.gen_range(12u8..255u8));
        let cs_eq = rsip::headers::CSeq::new(&cs_eq_str).into();
        headers.push(cs_eq);
        headers.push(rsip::headers::Event::new(format!("Catalog;id={}", rng.gen_range(123456789u32..987654321u32))).into());
        headers.push(rsip::headers::ContentType::new("Application/MANSCDP+xml").into());
        headers.push(rsip::headers::ContentLength::from(body.len() as u32).into());
        let request_msg: SipMessage = Request {
            method: Method::Subscribe,
            uri,
            headers,
            version: rsip::common::version::Version::V2,
            body: body.as_bytes().to_vec(),
        }.into();
        let ident = Ident::new(device_id.to_string(), call_id_str, cs_eq_str);
        Ok((ident, request_msg))
    }


    async fn common_info_request(device_id: &String, channel_id: &String, body: &str, from_tag: &str, to_tag: &str, seq: Option<u32>, call_id: Option<String>) -> GlobalResult<(Ident, SipMessage)> {
        let (mut headers, uri) = Self::build_request_header(Some(channel_id), device_id, false, true, Some(from_tag), Some(to_tag)).await?;
        let call_id = call_id.unwrap_or_else(|| Uuid::new_v4().as_simple().to_string());
        headers.push(rsip::headers::CallId::new(&call_id).into());
        let seq = seq.map(|seq| seq).unwrap_or_else(|| {
            let mut rng = thread_rng();
            rng.gen_range(12u32..255u32)
        });
        let cs_eq_str = format!("{} INFO", seq);
        let cs_eq = rsip::headers::CSeq::new(&cs_eq_str).into();
        headers.push(cs_eq);

        headers.push(rsip::headers::ContentType::new("Application/MANSRTSP").into());
        headers.push(rsip::headers::ContentLength::from(body.len() as u32).into());
        let msg = Request {
            method: Method::Info,
            uri,
            headers,
            version: rsip::common::version::Version::V2,
            body: body.as_bytes().to_vec(),
        }.into();
        let ident = Ident::new(device_id.to_string(), call_id, cs_eq_str);
        Ok((ident, msg))
    }

    /// 构建下发请求头
    async fn build_request_header(channel_id: Option<&String>, device_id: &String, expires: bool, contact: bool, from_tag: Option<&str>, to_tag: Option<&str>)
                                  -> GlobalResult<(rsip::Headers, Uri)> {
        let bill = RWSession::get_bill_by_device_id(device_id).ok_or(SysErr(anyhow!("设备：{device_id}，未注册或已离线"))).hand_log(|msg| warn!("{msg}"))?;
        let mut dst_id = device_id;
        if channel_id.is_some() {
            let channel_id = channel_id.unwrap();
            let channel_status = mapper::get_device_channel_status(device_id, channel_id).await?.ok_or(SysErr(anyhow!("设备：{device_id} - 通道：{channel_id}，未知或无效")))?;
            match &channel_status.to_ascii_uppercase()[..] {
                "OK" | "ON" | "ONLINE" | "ONLY" | "" => { dst_id = channel_id }
                _ => {
                    Err(SysErr(anyhow!("设备：{device_id} - 通道：{channel_id}，已下线")))?
                }
            }
        }
        let conf = SessionConf::get_session_by_conf();
        let server_ip = &conf.get_wan_ip().to_string();
        let server_port = conf.get_wan_port();
        //domain宜采用ID统一编码的前十位编码,扩展支持十位编码加“.spvmn.cn”后缀格式,或采用IP:port格式,port宜采用5060;这里统一使用device_id的前十位,不再调用DB进行判断原设备的使用方式
        // let gmv_device = GmvDevice::query_gmv_device_by_device_id(&device_id.to_string())?.ok_or(SysErr(anyhow!("设备：{device_id}-{channel_id:?}未注册或已离线。")))?;
        let oauth = GmvOauth::read_gmv_oauth_by_device_id(device_id).await?
            .ok_or(SysErr(anyhow!("device id = [{}] 未知设备",device_id)))
            .hand_log(|msg| warn!("{msg}"))?;
        if oauth.get_status() == &0u8 {
            warn!("device id = [{}] 未启用设备,无法下发指令",&device_id);
        }
        let domain_id = oauth.get_domain_id();
        let domain = &format!("{}.spvmn.cn", oauth.get_domain());

        let transport = bill.get_protocol().get_value();
        let uri_str = format!("sip:{}@{}", dst_id, domain);
        let uri = uri::Uri::try_from(uri_str).hand_log(|msg| warn!("{msg}"))?;
        let mut rng = StdRng::from_entropy();
        let mut headers: rsip::Headers = Default::default();
        headers.push(rsip::headers::Via::new(format!("SIP/2.0/{} {}:{};rport;branch=z9hG4bK{}", transport, server_ip, server_port, rng.gen_range(123456789u32..987654321u32))).into());
        headers.push(rsip::headers::From::new(format!("<sip:{}@{}>;tag={}", domain_id, domain, from_tag.unwrap_or(&rng.gen_range(123456789u32..987654321u32).to_string()))).into());
        to_tag.map(|tag| {
            headers.push(rsip::headers::To::new(format!("<sip:{}@{}>;tag={}", dst_id, domain, tag)).into())
        }).unwrap_or_else(
            || { headers.push(rsip::headers::To::new(format!("<sip:{}@{}>", dst_id, domain)).into()) }
        );
        if expires {
            let expires = RWSession::get_expires_by_device_id(device_id).ok_or(SysErr(anyhow!("device id = [{}] 未知设备",device_id)))?;
            headers.push(rsip::headers::Expires::new(expires.as_secs().to_string()).into());
        }
        if contact {
            headers.push(rsip::headers::Contact::new(format!("<sip:{}@{}:{}>", domain_id, server_ip, server_port)).into());
        }
        headers.push(rsip::headers::MaxForwards::new("70").into());
        headers.push(rsip::headers::UserAgent::new("GMV 0.1").into());
        Ok((headers, uri))
    }

    pub async fn play_live_request(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::play_live(channel_id, dst_ip, dst_port, stream_mode, ssrc)?;
        Self::build_stream_request(device_id, channel_id, ssrc, sdp).await
    }


    // 点播历史视频
    pub async fn playback(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String, st: u32, et: u32) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::playback(channel_id, dst_ip, dst_port, stream_mode, ssrc, st, et)?;
        Self::build_stream_request(device_id, channel_id, ssrc, sdp).await
    }

    // 云端录像
    pub async fn download(device_id: &String, channel_id: &String, dst_ip: &String, dst_port: u16, stream_mode: StreamMode, ssrc: &String, st: u32, et: u32, speed: u8) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::download(channel_id, dst_ip, dst_port, stream_mode, ssrc, st, et, speed)?;
        Self::build_stream_request(device_id, channel_id, ssrc, sdp).await
    }

    pub async fn build_bye_request(seq: u32, call_id: String, device_id: &String, channel_id: &String, from_tag: &str, to_tag: &str) -> GlobalResult<(Ident, SipMessage)> {
        let (mut headers, uri) = Self::build_request_header(Some(channel_id), device_id, false, true, Some(from_tag), Some(to_tag)).await?;
        headers.push(rsip::headers::CallId::new(&call_id).into());
        let cs_eq_str = format!("{} BYE", seq);
        let cs_eq = rsip::headers::CSeq::new(&cs_eq_str).into();
        headers.push(cs_eq);
        let msg = Request {
            method: Method::Bye,
            uri,
            headers,
            version: rsip::common::version::Version::V2,
            body: Default::default(),
        }.into();
        let ident = Ident::new(device_id.to_string(), call_id, cs_eq_str);
        Ok((ident, msg))
    }

    // 拍照
    // 云台控制
    // 查询硬盘录像情况
    // 拖动播放
    pub async fn seek(device_id: &String, channel_id: &String, seek: u32, from_tag: &str, to_tag: &str, seq: u32, call_id: String) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::info_seek(seek);
        Self::common_info_request(device_id, channel_id, &sdp, from_tag, to_tag, Some(seq), Some(call_id)).await
    }

    // 倍速播放
    pub async fn speed(device_id: &String, channel_id: &String, speed: f32, from_tag: &str, to_tag: &str, seq: u32, call_id: String) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::info_speed(speed);
        Self::common_info_request(device_id, channel_id, &sdp, from_tag, to_tag, Some(seq), Some(call_id)).await
    }

    // 暂停回放
    pub async fn pause(device_id: &String, channel_id: &String, from_tag: &str, to_tag: &str, seq: u32, call_id: String) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::info_pause();
        Self::common_info_request(device_id, channel_id, &sdp, from_tag, to_tag, Some(seq), Some(call_id)).await
    }

    // 恢复回放
    pub async fn replay(device_id: &String, channel_id: &String, from_tag: &str, to_tag: &str, seq: u32, call_id: String) -> GlobalResult<(Ident, SipMessage)> {
        let sdp = SdpBuilder::info_replay();
        Self::common_info_request(device_id, channel_id, &sdp, from_tag, to_tag, Some(seq), Some(call_id)).await
    }

    async fn build_stream_request(device_id: &String, channel_id: &String, ssrc: &str, body: String) -> GlobalResult<(Ident, SipMessage)> {
        let (mut headers, uri) = Self::build_request_header(Some(channel_id), device_id, false, true, None, None).await?;
        let call_id_str = Uuid::new_v4().as_simple().to_string();
        headers.push(rsip::headers::CallId::new(&call_id_str).into());
        let mut rng = thread_rng();
        let cs_eq_str = format!("{} INVITE", rng.gen_range(12u8..255u8));
        let cs_eq = rsip::headers::CSeq::new(&cs_eq_str).into();
        headers.push(cs_eq);

        let from: &headers::From = header!(
            headers.iter(),
            Header::From,
            Error::missing_header("From")
        ).hand_log(|msg| error!("{msg}"))?;
        headers.push(rsip::headers::Subject::new(format!("{}:{},{}:0", channel_id, ssrc, from.uri().unwrap().auth.unwrap().user)).into());
        headers.push(rsip::headers::ContentType::new("Application/SDP").into());
        headers.push(rsip::headers::ContentLength::from(body.len() as u32).into());
        let msg = Request {
            method: Method::Invite,
            uri,
            headers,
            version: rsip::common::version::Version::V2,
            body: body.as_bytes().to_vec(),
        }.into();
        let ident = Ident::new(device_id.to_string(), call_id_str, cs_eq_str);
        Ok((ident, msg))
    }

    pub fn build_ack_request_by_response(res: &Response) -> GlobalResult<SipMessage> {
        let mut headers: rsip::Headers = Default::default();
        headers.push(res.to_header().hand_log(|msg| warn!("{msg}"))?.clone().into());
        let from = res.from_header().hand_log(|msg| warn!("{msg}"))?;
        headers.push(from.clone().into());
        headers.push(res.call_id_header().hand_log(|msg| warn!("{msg}"))?.clone().into());
        let mut rng = thread_rng();
        let via: typed::Via = res.via_header().hand_log(|msg| warn!("{msg}"))?.typed().hand_log(|msg| warn!("{msg}"))?;
        headers.push(rsip::headers::Via::new(format!("SIP/2.0/{} {};rport;branch=z9hG4bK{}",
                                                     via.transport.to_string(),
                                                     via.uri.to_string(),
                                                     rng.gen_range(123456789u32..987654321u32))).into());
        let seq = res.cseq_header().hand_log(|msg| warn!("{msg}"))?.seq().hand_log(|msg| warn!("{msg}"))?;
        headers.push(rsip::headers::CSeq::new(format!("{seq} ACK")).into());
        let uri = from.uri().hand_log(|msg| warn!("{msg}"))?;
        let uri_str = uri.to_string();
        headers.push(rsip::headers::Contact::new(format!("<{uri_str}>")).into());
        headers.push(rsip::headers::MaxForwards::new("70").into());
        headers.push(rsip::headers::UserAgent::new("GMV 0.1").into());
        headers.push(rsip::headers::ContentLength::default().into());
        Ok(rsip::Request {
            method: Method::Ack,
            uri,
            headers,
            version: rsip::common::version::Version::V2,
            body: Default::default(),
        }.into())
    }
}

struct XmlBuilder;

///编码格式：2022 GB18030
/// 2016 GB2312
impl XmlBuilder {
    pub fn control_ptz(ptz_control_model: &PtzControlModel) -> String {
        let mut xml = String::with_capacity(200);
        xml.push_str("<?xml version=\"1.0\" encoding=\"GB18030\"?>\r\n");
        xml.push_str("<Control>\r\n");
        xml.push_str("<CmdType>DeviceControl</CmdType>\r\n");
        xml.push_str(&*format!("<SN>{}</SN>\r\n", Local::now().timestamp()));
        xml.push_str(&*format!("<DeviceID>{}</DeviceID>\r\n", ptz_control_model.channelId));
        xml.push_str(&*format!("<PTZCmd>{}</PTZCmd>\r\n", Self::build_cmd_ptz_line(ptz_control_model)));
        xml.push_str("<Info>\r\n");
        xml.push_str("<ControlPriority>5</ControlPriority>\r\n");
        xml.push_str("</Info>\r\n");
        xml.push_str("</Control>\r\n");
        xml
    }
    fn build_cmd_ptz_line(ptz_control_model: &PtzControlModel) -> String {
        //前三字节：
        // byte1: A5 start header 固定
        // byte2: 0F 组合码1 与版本信息相关 固定
        // byte3: 01 地址位 前端控制时可以不管 可使用01-FF 固定
        // let mut builder = String::from("A50F00");
        let mut bytes = [0xA5, 0x0F, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00];
        if ptz_control_model.leftRight == 2 {
            bytes[3] |= 0x01; // 右移
        } else if ptz_control_model.leftRight == 1 {
            bytes[3] |= 0x02; // 左移
        }

        if ptz_control_model.upDown == 2 {
            bytes[3] |= 0x04; // 下移
        } else if ptz_control_model.upDown == 1 {
            bytes[3] |= 0x08; // 上移
        }

        if ptz_control_model.inOut == 2 {
            bytes[3] |= 0x10; // 放大
        } else if ptz_control_model.inOut == 1 {
            bytes[3] |= 0x20; // 缩小
        }
        bytes[4] = ptz_control_model.horizonSpeed;
        bytes[5] = ptz_control_model.verticalSpeed;
        bytes[6] = ptz_control_model.zoomSpeed << 4;
        bytes[7] = (bytes.iter().copied().map(|x| x as u16).sum::<u16>() % 256) as u8;
        let mut cmd_line = String::new();
        for byte in &bytes {
            write!(&mut cmd_line, "{:02X}", byte).unwrap();
        }
        cmd_line
    }
    pub fn control_snapshot_image(channel_id: &String, num: u8, interval: u8, uri: &String, session_id: &String) -> String {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\"?>\r\n");
        xml.push_str("<Control>\r\n");
        xml.push_str("<CmdType>DeviceConfig</CmdType>\r\n");
        xml.push_str(&*format!("<SN>{}</SN>\r\n", Local::now().timestamp_subsec_millis()));
        xml.push_str(&*format!("<DeviceID>{}</DeviceID>\r\n", channel_id));
        xml.push_str("<SnapShotConfig>\r\n");
        xml.push_str(&*format!("<SnapNum>{}</SnapNum>\r\n", num));
        xml.push_str(&*format!("<Interval>{}</Interval>\r\n", interval));
        xml.push_str(&*format!("<UploadURL>{}</UploadURL>\r\n", uri));
        xml.push_str(&*format!("<SessionID>{}</SessionID>\r\n", session_id));
        xml.push_str("</SnapShotConfig>\r\n");
        xml.push_str("</Control>\r\n");
        xml
    }

    pub fn query_preset(device_id: &String) -> String {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n");
        xml.push_str("<Query>\r\n");
        xml.push_str("<CmdType>PresetQuery</CmdType>\r\n");
        xml.push_str(&*format!("<SN>{}</SN>\r\n", Local::now().timestamp_subsec_millis()));
        xml.push_str(&*format!("<DeviceID>{}</DeviceID>\r\n", device_id));
        xml.push_str("</Query>\r\n");
        xml
    }
    pub fn query_device_info(device_id: &String) -> String {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n");
        xml.push_str("<Query>\r\n");
        xml.push_str("<CmdType>DeviceInfo</CmdType>\r\n");
        xml.push_str(&*format!("<SN>{}</SN>\r\n", Local::now().timestamp_subsec_millis()));
        xml.push_str(&*format!("<DeviceID>{}</DeviceID>\r\n", device_id));
        xml.push_str("</Query>\r\n");
        xml
    }

    pub fn query_device_catalog(device_id: &String) -> String {
        let mut xml = String::new();
        xml.push_str("<?xml version=\"1.0\" encoding=\"GB2312\"?>\r\n");
        xml.push_str("<Query>\r\n");
        xml.push_str("<CmdType>Catalog</CmdType>\r\n");
        xml.push_str(&*format!("<SN>{}</SN>\r\n", Local::now().timestamp_subsec_millis()));
        xml.push_str(&*format!("<DeviceID>{}</DeviceID>\r\n", device_id));
        xml.push_str("</Query>\r\n");
        xml
    }
}

struct SdpBuilder;

#[allow(unused)]
impl SdpBuilder {
    pub fn info_pause() -> String {
        let mut sdp = String::with_capacity(100);
        sdp.push_str("PAUSE RTSP/1.0\r\n");
        sdp.push_str(&format!("CSeq: {}\r\n", Local::now().timestamp()));
        sdp.push_str("PauseTime: now\r\n");
        sdp
    }

    //暂停后恢复播放
    pub fn info_replay() -> String {
        let mut sdp = String::with_capacity(100);
        sdp.push_str("PLAY RTSP/1.0\r\n");
        sdp.push_str(&format!("CSeq: {}\r\n", Local::now().timestamp()));
        sdp.push_str("Range: npt=now-\r\n");
        sdp
    }

    //倍速播放
    pub fn info_speed(speed: f32) -> String {
        let mut sdp = String::with_capacity(100);
        sdp.push_str("PLAY RTSP/1.0\r\n");
        sdp.push_str(&format!("CSeq: {}\r\n", Local::now().timestamp()));
        sdp.push_str(&format!("Scale: {}.000000\r\n", speed));
        sdp
    }

    //拖动播放
    pub fn info_seek(seek: u32) -> String {
        let mut sdp = String::with_capacity(100);
        sdp.push_str("PLAY RTSP/1.0\r\n");
        sdp.push_str(&format!("CSeq: {}\r\n", Local::now().timestamp()));
        sdp.push_str(&format!("Range: npt={}-\r\n", seek));
        sdp
    }

    pub fn playback(channel_id: &String, media_ip: &String, media_port: u16, stream_mode: StreamMode, ssrc: &String, st: u32, et: u32) -> GlobalResult<String> {
        let st_et = format!("{} {}", st, et);
        let sdp = Self::build_common_play(channel_id, media_ip, media_port, stream_mode, ssrc, "Playback", &st_et, true, None)?;
        Ok(sdp)
    }

    pub fn download(channel_id: &String, media_ip: &String, media_port: u16, stream_mode: StreamMode, ssrc: &String, st: u32, et: u32, download_speed: u8) -> GlobalResult<String> {
        let st_et = format!("{} {}", st, et);
        let sdp = Self::build_common_play(channel_id, media_ip, media_port, stream_mode, ssrc, "Download", &st_et, true, Some(download_speed))?;
        Ok(sdp)
    }
    pub fn play_live(channel_id: &String, media_ip: &String, media_port: u16, stream_mode: StreamMode, ssrc: &String) -> GlobalResult<String> {
        let sdp = Self::build_common_play(channel_id, media_ip, media_port, stream_mode, ssrc, "Play", "0 0", false, None)?;
        Ok(sdp)
    }

    ///缺s:Play/Playback/Download; t:开始时间戳 结束时间戳; u:回放与下载时的取流地址
    fn build_common_play(channel_id: &String, media_ip: &String, media_port: u16, stream_mode: StreamMode, ssrc: &String, name: &str, st_et: &str, u: bool, download_speed: Option<u8>) -> GlobalResult<String> {
        let conf = SessionConf::get_session_by_conf();
        let session_ip = &conf.get_wan_ip().to_string();
        let mut sdp = String::with_capacity(300);
        sdp.push_str("v=0\r\n");
        sdp.push_str(&format!("o={} 0 0 IN IP4 {}\r\n", channel_id, session_ip));
        sdp.push_str(&format!("s={}\r\n", name));
        if u {
            sdp.push_str(&format!("u={}:0\r\n", channel_id));
        }
        sdp.push_str(&format!("c=IN IP4 {}\r\n", media_ip));
        sdp.push_str(&format!("t={}\r\n", st_et));
        match stream_mode {
            StreamMode::Udp => {
                sdp.push_str(&format!("m=video {} RTP/AVP 96 97 98 99 100\r\n", media_port))
            }
            StreamMode::TcpActive => {
                sdp.push_str(&format!("m=video {} TCP/RTP/AVP 96 97 98 99 100\r\n", media_port));
                sdp.push_str("a=setup:active\r\n");
                sdp.push_str("a=connection:new\r\n");
            }
            StreamMode::TcpPassive => {
                sdp.push_str(&format!("m=video {} TCP/RTP/AVP 96 97 98 99 100\r\n", media_port));
                sdp.push_str("a=setup:passive\r\n");
                sdp.push_str("a=connection:new\r\n");
            }
        }
        sdp.push_str("a=recvonly\r\n");
        sdp.push_str("a=rtpmap:96 PS/90000\r\n");
        sdp.push_str("a=rtpmap:97 MPEG4/90000\r\n");
        sdp.push_str("a=rtpmap:98 H264/90000\r\n");
        sdp.push_str("a=rtpmap:99 SVAC/90000\r\n");
        sdp.push_str("a=rtpmap:100 H265/90000\r\n");
        download_speed.map(|speed| sdp.push_str(&format!("a=downloadspeed:{}\r\n", speed)));
        sdp.push_str(&format!("y={}\r\n", ssrc));
        Ok(sdp)
    }
}

#[cfg(test)]
mod tests {
    use base::chrono::Local;
    use crate::state::model::PtzControlModel;

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

    #[test]
    fn test_x_code() {
        let msg = format!("{:x}0", 12);
        println!("{}", msg);
    }

    #[test]
    fn test_ptz_cmd() {
        let mut model = PtzControlModel {
            deviceId: "".to_string(),
            channelId: "".to_string(),
            leftRight: 0,
            upDown: 0,
            inOut: 0,
            horizonSpeed: 0,
            verticalSpeed: 0,
            zoomSpeed: 0,
        };
        let stop_cmd = super::XmlBuilder::build_cmd_ptz_line(&model);
        assert_eq!(stop_cmd, "A50F0100000000B5");
        {
            model.upDown = 1;
            model.verticalSpeed = 250;
            let up_cmd = super::XmlBuilder::build_cmd_ptz_line(&model);
            assert_eq!(up_cmd, "A50F010800FA00B7");
            model.upDown = 0;
            model.verticalSpeed = 0;
        }
        {
            model.inOut = 1;
            model.zoomSpeed = 10;
            let out_cmd = super::XmlBuilder::build_cmd_ptz_line(&model);
            assert_eq!(out_cmd, "A50F01200000A075");
        }
    }

    #[test]
    fn test_left_mv() {
        let sec = Local::now().timestamp();
        println!("+0 {}", sec);
        println!("+1 {}", sec + 1);
        println!("+2 {}", sec + 2);
        println!("+3 {}", sec + 3);
        println!("L0 {}", sec >> 1);
        println!("L1 {}", (sec + 1) >> 1);
        println!("L2 {}", (sec + 2) >> 1);
        println!("L3 {}", (sec + 3) >> 1);
    }
}