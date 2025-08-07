// use std::collections::HashMap;
// use std::net::Ipv4Addr;
// use std::time::Duration;
// 
// use anyhow::anyhow;
// use common::exception::{GlobalError, GlobalResult, GlobalResultExt};
// use common::exception::GlobalError::SysErr;
// use common::log::error;
// use common::serde::{Deserialize, Serialize};
// use reqwest::header;
// use reqwest::header::HeaderMap;
// 
// use crate::general::AlarmConf;
// use crate::general::model::AlarmInfo;
// use crate::service::{EXPIRES, ResMsg, StreamRecordInfo};
// 
// #[allow(dead_code)]
// const DROP_SSRC: &str = "/drop/ssrc";
// #[allow(dead_code)]
// const LISTEN_SSRC: &str = "/listen/ssrc";
// #[allow(dead_code)]
// const START_RECORD: &str = "/start/record";
// #[allow(dead_code)]
// const ON_RECORD: &str = "/on/record";
// #[allow(dead_code)]
// const START_PLAY: &str = "/start/play";
// #[allow(dead_code)]
// const STOP_PLAY: &str = "/stop/play";
// #[allow(dead_code)]
// const QUERY_STREAM_COUNT: &str = "/stream/count";
// #[allow(dead_code)]
// const RTP_MEDIA: &str = "/rtp/media";
// 
// fn build_uri_header(gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<(String, HeaderMap)> {
//     let uri = format!("http://{}:{}", local_ip.to_string(), local_port);
//     let mut headers = HeaderMap::new();
//     headers.insert("gmv-token", header::HeaderValue::from_str(gmv_token).hand_log(|msg| error!("{msg}"))?);
//     Ok((uri, headers))
// }
// 
// pub async fn get_stream_count(opt_stream_id: Option<&String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<u32> {
//     let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
//     if let Some(stream_id) = opt_stream_id {
//         uri = format!("{uri}{QUERY_STREAM_COUNT}?stream_id={stream_id}");
//     } else {
//         uri = format!("{uri}{QUERY_STREAM_COUNT}");
//     }
// 
//     let res = reqwest::Client::builder()
//         .timeout(Duration::from_secs(EXPIRES))
//         .default_headers(headers)
//         .build()
//         .hand_log(|msg| error!("{msg}"))?
//         .get(&uri)
//         .send()
//         .await
//         .hand_log(|msg| error!("{msg}"))?;
//     return if res.status().is_success() {
//         let body = res.json::<ResMsg<u32>>()
//             .await
//             .hand_log(|msg| error!("{msg}"))?;
//         if body.code == 200 {
//             if let Some(data) = body.data {
//                 return Ok(data);
//             }
//         }
//         Err(SysErr(anyhow!("{}",body.msg))).hand_log(|msg| error!("{msg}"))?
//     } else {
//         Err(SysErr(anyhow!("{}",res.status().to_string()))).hand_log(|msg| error!("{msg}"))?
//     };
// }
// 
// #[derive(Clone, Deserialize, Serialize, Debug, Default)]
// #[serde(crate = "common::serde")]
// pub struct HlsPiece {
//     //片时间长度 S
//     pub duration: u8,
//     pub live: bool,
// }
// 
// #[derive(Clone, Serialize, Deserialize, Debug)]
// #[serde(crate = "common::serde")]
// pub enum Download {
//     //录像 filename,type
//     Mp4(String, Option<String>),
//     //截图 filename
//     Picture(String, Option<String>),
// }
// 
// #[derive(Clone, Serialize, Deserialize, Debug)]
// #[serde(crate = "common::serde")]
// pub enum Play {
//     Flv,
//     Hls(HlsPiece),
//     FlvHls(HlsPiece),
// }
// 
// #[derive(Clone, Serialize, Deserialize, Debug)]
// #[serde(crate = "common::serde")]
// pub enum MediaAction {
//     //点播
//     Play(Play),
//     //下载
//     Download(Download),
// }
// 
// #[derive(Deserialize, Serialize, Debug)]
// #[serde(crate = "common::serde")]
// pub struct SsrcLisDto {
//     pub ssrc: u32,
//     pub stream_id: String,
//     //当为None时，默认配置,负数-立马关闭
//     pub expires: Option<i32>,
//     pub media_action: MediaAction,
//     // pub flv: bool,
//     // pub hls: Option<HlsDto>,
//     //MP4录像保存，filename
//     // pub mp4: Option<String>
// }
// 
// pub async fn call_listen_ssrc(stream_id: String, ssrc: &String, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16, media_action: MediaAction) -> GlobalResult<bool> {
//     let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
//     let ssrc_lis_dto = SsrcLisDto {
//         ssrc,
//         stream_id,
//         media_action,
//         expires: None,
//     };
//     let (mut uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
//     uri = format!("{uri}{LISTEN_SSRC}");
//     let res = reqwest::Client::builder()
//         .timeout(Duration::from_secs(EXPIRES))
//         .default_headers(headers)
//         .build()
//         .hand_log(|msg| error!("{msg}"))?
//         .post(&uri)
//         .json(&ssrc_lis_dto)
//         .send()
//         .await
//         .hand_log(|msg| error!("{msg}"))?;
//     return if res.status().is_success() {
//         let body = res.json::<ResMsg<bool>>()
//             .await
//             .hand_log(|msg| error!("{msg}"))?;
//         Ok(body.code == 200)
//     } else {
//         Err(SysErr(anyhow!("{}",res.status().to_string()))).hand_log(|msg| error!("{msg}"))?
//     };
// }
// 
// #[derive(Serialize, Deserialize, Debug)]
// #[serde(crate = "common::serde")]
// struct RtpMap {
//     ssrc: u32,
//     map: HashMap<u8, String>,
// }
// 
// pub async fn ident_rtp_media_info(ssrc: &String, map: HashMap<u8, String>, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<bool> {
//     let ssrc = ssrc.parse::<u32>().hand_log(|msg| error!("{msg}"))?;
//     let rtp_map = RtpMap { ssrc, map };
//     let (uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
//     let res = reqwest::Client::builder()
//         .timeout(Duration::from_secs(EXPIRES))
//         .default_headers(headers)
//         .build()
//         .hand_log(|msg| error!("{msg}"))?
//         .post(format!("{uri}{RTP_MEDIA}"))
//         .json(&rtp_map)
//         .send()
//         .await
//         .hand_log(|msg| error!("{msg}"))?;
//     return if res.status().is_success() {
//         let body = res.json::<ResMsg<bool>>()
//             .await
//             .hand_log(|msg| error!("{msg}"))?;
//         Ok(body.code == 200)
//     } else {
//         Err(SysErr(anyhow!("{}",res.status().to_string()))).hand_log(|msg| error!("{msg}"))?
//     };
// }
// 
// 
// pub async fn get_stream_record_info_by_biz_id(stream_id: &String, gmv_token: &String, local_ip: &Ipv4Addr, local_port: &u16) -> GlobalResult<StreamRecordInfo> {
//     let (uri, headers) = build_uri_header(gmv_token, local_ip, local_port)?;
//     let res = reqwest::Client::builder()
//         .timeout(Duration::from_secs(EXPIRES))
//         .default_headers(headers)
//         .build()
//         .hand_log(|msg| error!("{msg}"))?
//         .get(format!("{}{}?stream_id={}", uri, ON_RECORD, stream_id))
//         .send()
//         .await
//         .hand_log(|msg| error!("{msg}"))?;
//     return if res.status().is_success() {
//         let body = res.json::<ResMsg<StreamRecordInfo>>()
//             .await
//             .hand_log(|msg| error!("{msg}"))?;
//         body.data.ok_or_else(|| GlobalError::new_sys_error("record info 为空", |msg| error!("{msg}")))
//     } else {
//         Err(SysErr(anyhow!("{}",res.status().to_string()))).hand_log(|msg| error!("{msg}"))?
//     };
// }
// 
// pub async fn call_alarm_info(info: &AlarmInfo) -> GlobalResult<bool> {
//     let conf = AlarmConf::get_alarm_conf();
//     let res = reqwest::Client::builder()
//         .timeout(Duration::from_secs(EXPIRES))
//         .build()
//         .hand_log(|msg| error!("{msg}"))?
//         .post(conf.push_url.as_ref().unwrap())
//         .json(info)
//         .send()
//         .await
//         .hand_log(|msg| error!("{msg}"))?;
//     return if res.status().is_success() {
//         let body = res.json::<ResMsg<bool>>()
//             .await
//             .hand_log(|msg| error!("{msg}"))?;
//         Ok(body.code == 200)
//     } else {
//         Err(SysErr(anyhow!("{}",res.status().to_string()))).hand_log(|msg| error!("{msg}"))?
//     };
// }
// 
