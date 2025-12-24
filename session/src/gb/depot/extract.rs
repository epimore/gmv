use base::exception::{GlobalError, GlobalResult, GlobalResultExt};
use base::log::{error, warn};
use rsip::headers::{CSeq, CallId};
use rsip::message::HeadersExt;
use rsip::param::{Branch, Tag};
use rsip::{Method, Param};

pub trait HeaderItemExt: HeadersExt {
    fn branch(&self) -> GlobalResult<Branch> {
        self.via_header()
            .hand_log(|msg| warn!("{msg}"))?
            .params()
            .hand_log(|msg| warn!("{msg}"))?
            .iter()
            .find_map(|p| match p {
                Param::Branch(b) => Some(b.clone()),
                _ => None,
            })
            .ok_or_else(|| {
                GlobalError::new_sys_error(
                    &format!("Via miss branch: {:?} ", self.via_header()),
                    |msg| error!("{msg}"),
                )
            })
    }
    fn cs_eq(&self) -> GlobalResult<&CSeq> {
        self.cseq_header().hand_log(|msg| warn!("{msg}"))
    }

    fn seq(&self) -> GlobalResult<u32> {
        self.cs_eq()?.seq().hand_log(|msg| warn!("{msg}"))
    }

    fn method_by_cseq(&self) -> GlobalResult<Method> {
        self.cs_eq()?.method().hand_log(|msg| warn!("{msg}"))
    }

    fn call_id(&self) -> GlobalResult<&CallId> {
        self.call_id_header().hand_log(|msg| warn!("{msg}"))
    }

    fn header_from_tag(&self) -> GlobalResult<Tag> {
        self.from_header()
            .hand_log(|msg| warn!("{msg}"))?
            .tag()
            .hand_log(|msg| warn!("{msg}"))?
            .ok_or_else(|| {
                GlobalError::new_sys_error(
                    &format!("From miss tag: {:?} ", self.from_header()),
                    |msg| error!("{msg}"),
                )
            })
    }

    //request 非必带
    //response【除100 Trying响应不能有To Tag】必带
    fn header_to_tag(&self) -> GlobalResult<Option<Tag>> {
        let tag = self
            .to_header()
            .hand_log(|msg| warn!("{msg}"))?
            .tag()
            .hand_log(|msg| warn!("{msg}"))?;
        Ok(tag)
    }
}

impl HeaderItemExt for rsip::Request {}
impl HeaderItemExt for rsip::Response {}
impl HeaderItemExt for rsip::SipMessage {}

