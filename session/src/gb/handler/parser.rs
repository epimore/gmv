pub mod header {
    use anyhow::anyhow;
    use base::exception::GlobalError::SysErr;
    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::warn;
    use rsip::headers::Via;
    use rsip::prelude::{HasHeaders, HeadersExt};
    use rsip::{Header, Param, Request, Response};

    pub fn get_device_id_by_request(req: &Request) -> GlobalResult<String> {
        let from_user = req
            .from_header()
            .hand_log(|msg| warn!("{msg}"))?
            .uri()
            .hand_log(|msg| warn!("{msg}"))?
            .auth
            .ok_or(SysErr(anyhow!("user is none")))
            .hand_log(|msg| warn!("{msg}"))?
            .user;
        Ok(from_user)
    }

    pub fn get_device_id_by_response(req: &Response) -> GlobalResult<String> {
        let from_user = req
            .to_header()
            .hand_log(|msg| warn!("{msg}"))?
            .uri()
            .hand_log(|msg| warn!("{msg}"))?
            .auth
            .ok_or(SysErr(anyhow!("user is none")))
            .hand_log(|msg| warn!("{msg}"))?
            .user;
        Ok(from_user)
    }

    pub fn get_via_header(req: &Request) -> GlobalResult<&Via> {
        let via = req.via_header().hand_log(|msg| warn!("{msg}"))?;
        Ok(via)
    }

    pub fn get_transport(req: &Request) -> GlobalResult<String> {
        let transport = get_via_header(req)?
            .trasnport()
            .hand_log(|msg| warn!("{msg}"))?
            .to_string();
        Ok(transport)
    }

    pub fn get_local_addr(req: &Request) -> GlobalResult<String> {
        let local_addr = get_via_header(req)?
            .uri()
            .hand_log(|msg| warn!("{msg}"))?
            .host_with_port
            .to_string();
        Ok(local_addr)
    }
    pub fn enable_lr(req: &Request) -> GlobalResult<u8> {
        let contact = req.contact_header().hand_log(|msg| warn!("{msg}"))?;
        if let Ok(ps) = contact.params() {
            if ps.iter().any(|param| param == &Param::Lr) {
                return Ok(1);
            }
        }
        Ok(0)
    }

    pub fn get_contact_uri(req: &Request) -> GlobalResult<String> {
        let contact = req.contact_header().hand_log(|msg| warn!("{msg}"))?;
        Ok(contact.uri().hand_log(|msg| warn!("{msg}"))?.to_string())
    }
    // pub fn get_from(req: &Request) -> GlobalResult<String> {
    //     let from = req.from_header().hand_log(|msg| warn!("{msg}"))?.uri().hand_log(|msg| warn!("{msg}"))?.to_string();
    //     Ok(from)
    // }
    //
    // pub fn get_to(req: &Request) -> GlobalResult<String> {
    //     let to = req.to_header().hand_log(|msg| warn!("{msg}"))?.uri().hand_log(|msg| warn!("{msg}"))?.to_string();
    //     Ok(to)
    // }

    pub fn get_expires(req: &Request) -> GlobalResult<u32> {
        let expires = req
            .expires_header()
            .ok_or(SysErr(anyhow!("无参数expires")))
            .hand_log(|msg| warn!("{msg}"))?
            .seconds()
            .hand_log(|msg| warn!("{msg}"))?;
        Ok(expires)
    }

    pub fn get_domain(req: &Request) -> GlobalResult<String> {
        let to_uri = req
            .to_header()
            .hand_log(|msg| warn!("{msg}"))?
            .uri()
            .hand_log(|msg| warn!("{msg}"))?;
        Ok(to_uri.host_with_port.to_string())
    }

    pub fn get_gb_version(req: &Request) -> Option<String> {
        for header in req.headers().iter() {
            match header {
                Header::Other(key, val) => {
                    if key.eq("X-GB-Ver") {
                        return Some(val.to_string());
                        // return match &val[..] {
                        //     "1.0" => Some("GB/T 28181—2011".to_string()),
                        //     "1.1" => Some("GB/T 28181—2011-1".to_string()),
                        //     "2.0" => Some("GB/T 28181—2016".to_string()),
                        //     "3.0" => Some("GB/T 28181—2022".to_string()),
                        //     &_ => Some(val.to_string())
                        // };
                    }
                }
                _ => {
                    continue;
                }
            };
        }
        None
    }
}

pub mod xml {
    use anyhow::anyhow;
    use base::exception::GlobalError::SysErr;
    use base::exception::{GlobalResult, GlobalResultExt};
    use base::log::{debug, error};
    use encoding_rs::GB18030;
    use quick_xml::events::Event;
    use quick_xml::{Reader, encoding};
    use std::ops::Deref;
    use std::str::from_utf8;

    pub const MESSAGE_TYPE: [&'static str; 4] = [
        "Query,CmdType",
        "Control,CmdType",
        "Response,CmdType",
        "Notify,CmdType",
    ];
    pub const MESSAGE_KEEP_ALIVE: &str = "Keepalive";
    pub const MESSAGE_CONFIG_DOWNLOAD: &str = "ConfigDownload";
    pub const MESSAGE_NOTIFY_CATALOG: &str = "Catalog";
    pub const MESSAGE_DEVICE_INFO: &str = "DeviceInfo";
    pub const MESSAGE_ALARM: &str = "Alarm";
    pub const MESSAGE_RECORD_INFO: &str = "RecordInfo";
    pub const MESSAGE_UPLOAD_SNAPSHOT_FINISHED: &str = "UploadSnapShotFinished";
    pub const MESSAGE_UPLOAD_SNAPSHOT_SESSION_ID: &str = "Notify,SessionID";
    pub const MESSAGE_MEDIA_STATUS: &str = "MediaStatus";
    pub const MESSAGE_BROADCAST: &str = "Broadcast";
    pub const MESSAGE_DEVICE_STATUS: &str = "DeviceStatus";
    pub const MESSAGE_DEVICE_CONTROL: &str = "DeviceControl";
    pub const MESSAGE_DEVICE_CONFIG: &str = "DeviceConfig";
    pub const MESSAGE_PRESET_QUERY: &str = "PresetQuery";
    pub const RESPONSE_DEVICE_ID: &str = "Response,DeviceID";
    pub const RESPONSE_MANUFACTURER: &str = "Response,Manufacturer";
    pub const RESPONSE_MODEL: &str = "Response,Model";
    pub const RESPONSE_FIRMWARE: &str = "Response,Firmware";
    pub const RESPONSE_DEVICE_TYPE: &str = "Response,DeviceType";
    pub const RESPONSE_MAX_CAMERA: &str = "Response,MaxCamera";
    pub const RESPONSE_DEVICE_LIST_ITEM_DEVICE_ID: &str = "Response,DeviceList,Item,DeviceID";
    pub const RESPONSE_DEVICE_LIST_ITEM_NAME: &str = "Response,DeviceList,Item,Name";
    pub const RESPONSE_DEVICE_LIST_ITEM_MANUFACTURER: &str =
        "Response,DeviceList,Item,Manufacturer";
    pub const RESPONSE_DEVICE_LIST_ITEM_MODEL: &str = "Response,DeviceList,Item,Model";
    pub const RESPONSE_DEVICE_LIST_ITEM_OWNER: &str = "Response,DeviceList,Item,Owner";
    pub const RESPONSE_DEVICE_LIST_ITEM_CIVIL_CODE: &str = "Response,DeviceList,Item,CivilCode";
    pub const RESPONSE_DEVICE_LIST_ITEM_BLOCK: &str = "Response,DeviceList,Item,Block";
    pub const RESPONSE_DEVICE_LIST_ITEM_ADDRESS: &str = "Response,DeviceList,Item,Address";
    pub const RESPONSE_DEVICE_LIST_ITEM_PARENTAL: &str = "Response,DeviceList,Item,Parental";
    pub const RESPONSE_DEVICE_LIST_ITEM_PARENT_ID: &str = "Response,DeviceList,Item,ParentID";
    pub const RESPONSE_DEVICE_LIST_ITEM_LONGITUDE: &str = "Response,DeviceList,Item,Longitude";
    pub const RESPONSE_DEVICE_LIST_ITEM_LATITUDE: &str = "Response,DeviceList,Item,Latitude";
    pub const RESPONSE_DEVICE_LIST_ITEM_PTZ_TYPE: &str = "Response,DeviceList,Item,Info,PTZType";
    pub const RESPONSE_DEVICE_LIST_ITEM_SUPPLY_LIGHT_TYPE: &str =
        "Response,DeviceList,Item,SupplyLightType";
    pub const RESPONSE_DEVICE_LIST_ITEM_IP_ADDRESS: &str = "Response,DeviceList,Item,IPAddress";
    pub const RESPONSE_DEVICE_LIST_ITEM_PORT: &str = "Response,DeviceList,Item,Port";
    pub const RESPONSE_DEVICE_LIST_ITEM_PASSWORD: &str = "Response,DeviceList,Item,Password";
    pub const RESPONSE_DEVICE_LIST_ITEM_STATUS: &str = "Response,DeviceList,Item,Status";
    pub const SPLIT_CLASS: &str = "?<-0_0->?";
    pub const NOTIFY_DEVICE_ID: &str = "Notify,DeviceID";
    pub const NOTIFY_STATUS: &str = "Notify,Status";
    pub const NOTIFY_TYPE: &str = "Notify,NotifyType";
    pub const NOTIFY_UPLOAD_SNAP_SHOT_FINISHED: &str = "UploadSnapShotFinished";

    pub const NOTIFY_ALARM_PRIORITY: &str = "Notify,AlarmPriority";
    pub const NOTIFY_ALARM_TIME: &str = "Notify,AlarmTime";
    pub const NOTIFY_ALARM_METHOD: &str = "Notify,AlarmMethod";
    pub const NOTIFY_INFO_ALARM_TYPE: &str = "Notify,Info,AlarmType";

    pub fn parse_xlm_to_vec(xml: &[u8]) -> GlobalResult<Vec<(String, String)>> {
        let mut xml_reader = Reader::from_reader(xml);
        xml_reader.trim_text(true);
        xml_reader.expand_empty_elements(true);
        let mut vec: Vec<(String, String)> = Vec::new();
        let mut tag: String = String::new();
        let mut k = 0;
        let mut j = false;
        let mut b = false;
        loop {
            match xml_reader.read_event() {
                Ok(Event::Start(ref e)) => {
                    let start_tag = from_utf8(e.name().0).hand_log(|msg| error!("{msg}"))?;
                    tag.push_str(&*format!("{},", start_tag));
                    b = false;
                }
                Ok(Event::Text(e)) => {
                    //此处使用GB18030进行解析,兼容新版本要求
                    let val =
                        encoding::decode(e.deref(), GB18030).hand_log(|msg| error!("{msg}"))?;
                    let len = tag.split(",").collect::<Vec<&str>>().len() - 1;
                    if k != len || j {
                        k = len;
                        vec.push(("?<-0_0->?".to_string(), k.to_string()));
                    }
                    let key = tag[..tag.len() - 1].to_string();
                    vec.push((key, val.to_string()));
                    b = false;
                }
                Ok(Event::End(ref e)) => {
                    let end = tag.len() - e.len() - 1;
                    tag = tag[0..end].parse().hand_log(|msg| error!("{msg}"))?;
                    j = b;
                    b = true;
                }
                Err(e) => Err(SysErr(anyhow!(
                    "Error at position {}: {:?}",
                    xml_reader.buffer_position(),
                    e
                )))?,
                Ok(Event::Eof) => break,
                _ => (),
            }
        }
        debug!("{:?}", &vec);
        Ok(vec)
    }

    pub trait KV2Model {
        fn kv_to_model(arr: Vec<(String, String)>) -> GlobalResult<Self>
        where
            Self: Sized;
    }
}
