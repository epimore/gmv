//! LEGACY RSIP PIPELINE
//!
//! The medium-term SIP stack has moved to `crate::gb::sip` + `gmv_pjsip`.
//! This file is kept temporarily for compatibility with existing service APIs
//! and for migration reference. New code must not add SIP parsing, transaction,
//! dialog, CSeq/tag/branch, or header-generation logic here.
//!

use crate::gb::depot::extract::HeaderItemExt;
use crate::gb::io::send_sip_pkt_out;
use base::bytes::Bytes;
use base::exception::{GlobalError, GlobalResult};
use base::log::error;
use base::net::state::{Association, Zip};
use base::serde::Serialize;
use base::tokio::sync::mpsc::Sender;
use parking_lot::RwLock;
use rsip::headers::UntypedHeader;
use rsip::{Method, Request, Response};
use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

//最大缓存数量
const MAX_ANTI_REPLAY_SIZE: usize = 1000 * 1024;
//宽松策略8秒，且响应相同内容
const LOOSE_POLICY_TTL: Duration = Duration::from_secs(32);
//严格策略1分钟，且不做响应
const STRICT_POLICY_TTL: Duration = Duration::from_secs(60);

/// 防重放策略类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(crate = "base::serde")]
pub enum AntiReplayPolicy {
    /// 宽松防重放 - 用于幂等操作
    Loose {
        /// 宽松策略的缓存时间 8秒，返回相同响应内容
        cache_ttl: Duration,
    },
    /// 严格防重放 - 用于非幂等操作
    Strict {
        /// 严格策略的缓存时间 1分钟，静默丢弃不响应
        cache_ttl: Duration,
    },
}
impl AntiReplayPolicy {
    /// 根据SIP请求确定防重放策略
    pub fn policy_by_request(request: &Request) -> GlobalResult<AntiReplayPolicy> {
        let policy = match request.method_by_cseq()? {
            // === 宽松策略（幂等操作）===
            Method::Register => {
                // 注册请求：允许重试保证在线状态
                AntiReplayPolicy::Loose {
                    cache_ttl: LOOSE_POLICY_TTL,
                }
            }
            Method::Options => {
                // 选项查询：信息查询类
                AntiReplayPolicy::Loose {
                    cache_ttl: LOOSE_POLICY_TTL,
                }
            }
            Method::Subscribe => {
                // 订阅请求：状态订阅
                AntiReplayPolicy::Loose {
                    cache_ttl: LOOSE_POLICY_TTL,
                }
            }
            Method::Notify => {
                // 通知请求：状态推送
                AntiReplayPolicy::Loose {
                    cache_ttl: LOOSE_POLICY_TTL,
                }
            }

            // === 严格策略（非幂等操作）===
            Method::Invite => {
                // 邀请请求：建立媒体会话，防止重复建立
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::Ack => {
                // ACK确认：特殊处理，防止重复确认
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::Bye => {
                // 结束会话：防止重复结束
                AntiReplayPolicy::Loose {
                    cache_ttl: LOOSE_POLICY_TTL,
                }
            }
            Method::Cancel => {
                // 取消请求：防止重复取消
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::Message => {
                // 根据消息内容进一步判断
                Self::classify_message_policy(request)
            }
            Method::Info => {
                // INFO消息：通常用于会话中信息传递
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::PRack => {
                // 临时响应确认
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::Publish => {
                // 发布信息：可能改变状态
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::Refer => {
                // 引用转移：改变会话状态
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
            Method::Update => {
                // 更新会话：修改会话参数
                AntiReplayPolicy::Strict {
                    cache_ttl: STRICT_POLICY_TTL,
                }
            }
        };

        Ok(policy)
    }
    /// 根据MESSAGE消息内容进一步分类策略
    fn classify_message_policy(request: &Request) -> AntiReplayPolicy {
        // 尝试从消息体中解析业务类型
        if let body = request.body() {
            if let Ok(body_str) = std::str::from_utf8(body) {
                // GB/T 28181 MANSCDP XML消息解析
                if body_str.contains("<CmdType>Keepalive</CmdType>") {
                    // 心跳消息：宽松策略
                    return AntiReplayPolicy::Loose {
                        cache_ttl: LOOSE_POLICY_TTL,
                    };
                }
                if body_str.contains("<CmdType>Alarm</CmdType>") {
                    // 报警消息：严格策略，防止重复报警
                    return AntiReplayPolicy::Strict {
                        cache_ttl: STRICT_POLICY_TTL,
                    };
                }
                if body_str.contains("<CmdType>DeviceStatus</CmdType>") {
                    // 设备状态：宽松策略
                    return AntiReplayPolicy::Loose {
                        cache_ttl: LOOSE_POLICY_TTL,
                    };
                }
                if body_str.contains("<CmdType>DeviceInfo</CmdType>") {
                    // 设备信息查询：宽松策略
                    return AntiReplayPolicy::Loose {
                        cache_ttl: LOOSE_POLICY_TTL,
                    };
                }
                if body_str.contains("<CmdType>DeviceControl</CmdType>") {
                    // 设备控制：严格策略
                    return AntiReplayPolicy::Strict {
                        cache_ttl: STRICT_POLICY_TTL,
                    };
                }
                if body_str.contains("<CmdType>ConfigDownload</CmdType>") {
                    // 配置下载：严格策略
                    return AntiReplayPolicy::Strict {
                        cache_ttl: STRICT_POLICY_TTL,
                    };
                }
            }
        }
        // 默认严格策略（安全优先）
        AntiReplayPolicy::Strict {
            cache_ttl: STRICT_POLICY_TTL,
        }
    }
}

/// 防重放 key
pub trait AntiReplay: Send + Sync + HeaderItemExt {
    //call_id+cseq(seq+method)+from_tag+from_network
    fn generate_anti_key(&self, from_network: &str) -> GlobalResult<String> {
        Ok(format!(
            "{}:{}:{}:{}",
            self.call_id()?.value(),
            self.cs_eq()?.value(),
            self.header_from_tag()?.value(),
            from_network
        ))
    }
}
impl AntiReplay for rsip::Request {}
impl AntiReplay for rsip::Response {}
impl AntiReplay for rsip::SipMessage {}

/// 扩展的重复请求处理结果
pub enum AntiReplayKind {
    /// 需要正常处理业务逻辑
    NeedProcess,
    /// 使用缓存的响应内容回复
    RespondWithCached(Bytes),
    /// 静默丢弃，不发送任何响应
    /// 请求已加入排队，等待处理完成
    QueuedForProcessing,
}
struct Shard {
    //key : (AntiReplayPolicy,request_count,Option<Response>)
    anti_map: HashMap<String, (AntiReplayPolicy, usize, Option<Bytes>)>,
    expire_set: BTreeSet<(Instant, String)>,
}
pub struct AntiReplayContext {
    pub shard: Arc<RwLock<Shard>>,
}
impl AntiReplayContext {
    pub fn process_request(
        &self,
        output: &Sender<Zip>,
        request: &Request,
        association: Association,
    ) -> GlobalResult<bool> {
        if let Ok(kind) = self.handle_request(request, &association.remote_addr.to_string()) {
            match kind {
                AntiReplayKind::NeedProcess => {
                    return Ok(true);
                }
                AntiReplayKind::RespondWithCached(res) => {
                    send_sip_pkt_out(&output, res, association, Some("Anti"));
                }
                AntiReplayKind::QueuedForProcessing => {}
            }
        }
        Ok(false)
    }
    fn handle_request(
        &self,
        request: &Request,
        from_network: &str,
    ) -> GlobalResult<AntiReplayKind> {
        self.clean();
        let key = request.generate_anti_key(from_network)?;
        let mut shard = self.shard.write();
        Self::enforce_capacity(&mut shard);
        match shard.anti_map.entry(key) {
            Entry::Occupied(mut occ) => {
                let (_, count, res) = occ.get_mut();
                match res {
                    None => {
                        *count += 1;
                        Ok(AntiReplayKind::QueuedForProcessing)
                    }
                    Some(msg) => Ok(AntiReplayKind::RespondWithCached(msg.clone())),
                }
            }
            Entry::Vacant(vac) => {
                let policy = AntiReplayPolicy::policy_by_request(request)?;
                let now = Instant::now();
                let duration = match policy {
                    AntiReplayPolicy::Loose { cache_ttl } => cache_ttl,
                    AntiReplayPolicy::Strict { cache_ttl } => cache_ttl,
                };
                let expire = now + duration;
                let key = vac.key().to_string();
                vac.insert((policy, 1, None));
                shard.expire_set.insert((expire, key));
                Ok(AntiReplayKind::NeedProcess)
            }
        }
    }

    ///将响应信息添加到缓存，并返回原始请求次数；
    /// 一个请求亦可多次响应，1xx:临时响应；2xx-6xx:最终响应
    pub fn process_response(&self, from_network: &str, response: Response) -> GlobalResult<usize> {
        self.clean();
        let key = response.generate_anti_key(from_network)?;
        let mut shard = self.shard.write();
        match shard.anti_map.entry(key) {
            Entry::Occupied(mut occ) => {
                let (_, count, res) = occ.get_mut();
                *res = Some(Bytes::from(response));
                Ok(*count)
            }
            Entry::Vacant(va) => Err(GlobalError::new_sys_error("未知或超时响应", |msg| {
                error!("{}:{msg}", va.key())
            }))?,
        }
    }
    fn clean(&self) {
        let cutoff = (Instant::now(), String::new());
        let mut shard = self.shard.write();
        let shard = &mut *shard;
        let unexpired = shard.expire_set.split_off(&(cutoff));
        for (_, key) in shard.expire_set.iter() {
            shard.anti_map.remove(key);
        }
        shard.expire_set = unexpired;
    }
    fn enforce_capacity(shard: &mut Shard) {
        while shard.anti_map.len() >= MAX_ANTI_REPLAY_SIZE {
            let Some((_, key)) = shard.expire_set.pop_first() else {
                break;
            };
            shard.anti_map.remove(&key);
        }
    }
    pub fn init() -> Self {
        let shard = Shard {
            anti_map: HashMap::with_capacity(1024),
            expire_set: Default::default(),
        };
        Self {
            shard: Arc::new(RwLock::new(shard)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AntiReplayContext, AntiReplayKind};
    use rsip::headers::UntypedHeader;
    use rsip::{Method, Request, Response, StatusCode, Uri, Version};

    fn invite_request() -> Request {
        Request {
            method: Method::Invite,
            uri: Uri::try_from("sip:device@example.com").unwrap(),
            headers: vec![
                rsip::headers::Via::new(
                    "SIP/2.0/UDP platform.example.com;branch=z9hG4bK-invite",
                )
                .into(),
                rsip::headers::From::new(
                    "<sip:platform@example.com>;tag=platform-tag",
                )
                .into(),
                rsip::headers::To::new("<sip:device@example.com>").into(),
                rsip::headers::CallId::new("call-id").into(),
                rsip::headers::CSeq::new("1 INVITE").into(),
                rsip::headers::ContentLength::default().into(),
            ]
            .into(),
            version: Version::V2,
            body: Default::default(),
        }
    }

    fn invite_response() -> Response {
        Response {
            status_code: StatusCode::MethodNotAllowed,
            headers: vec![
                rsip::headers::Via::new(
                    "SIP/2.0/UDP platform.example.com;branch=z9hG4bK-invite",
                )
                .into(),
                rsip::headers::From::new(
                    "<sip:platform@example.com>;tag=platform-tag",
                )
                .into(),
                rsip::headers::To::new(
                    "<sip:device@example.com>;tag=device-tag",
                )
                .into(),
                rsip::headers::CallId::new("call-id").into(),
                rsip::headers::CSeq::new("1 INVITE").into(),
                rsip::headers::ContentLength::default().into(),
            ]
            .into(),
            version: Version::V2,
            body: Default::default(),
        }
    }

    #[test]
    fn strict_request_retransmission_waits_then_reuses_cached_response() {
        let context = AntiReplayContext::init();
        let request = invite_request();

        assert!(matches!(
            context.handle_request(&request, "192.0.2.10:5060").unwrap(),
            AntiReplayKind::NeedProcess
        ));
        assert!(matches!(
            context.handle_request(&request, "192.0.2.10:5060").unwrap(),
            AntiReplayKind::QueuedForProcessing
        ));

        assert_eq!(
            context
                .process_response("192.0.2.10:5060", invite_response())
                .unwrap(),
            2
        );
        assert!(matches!(
            context.handle_request(&request, "192.0.2.10:5060").unwrap(),
            AntiReplayKind::RespondWithCached(_)
        ));
    }
}
