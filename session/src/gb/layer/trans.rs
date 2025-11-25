use crate::gb::layer::extract::HeaderItemExt;
use base::exception::GlobalResult;
use parking_lot::RwLock;
use rsip::headers::UntypedHeader;
use rsip::{Method, Request, SipMessage};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::Instant;
/// 需要完整事务管理的业务类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransactionRequired {
    // === 必须事务管理的业务 ===

    // 媒体会话类 - 必须严格事务管理
    PlayLive,           // 实时点播 - 必须事务管理
    PlayBack,           // 录像回放 - 必须事务管理
    AudioBroadcast,     // 语音广播 - 必须事务管理
    AudioIntercom,      // 语音对讲 - 必须事务管理

    // 控制类 - 需要事务确认
    Control,            // 设备控制 - 需要确认
    SoftUpdate,         // 软件升级 - 需要确认
    SnapImage,          // 图像抓拍 - 需要确认

    // === 不需要或简化事务管理的业务 ===

    // 状态类 - 可以无状态处理
    Register,           // 注册 - 周期性，不需要严格事务
    AlarmPush,          // 告警推送 - 事件性，不需要响应事务
    QueryInfo,          // 信息查询 - 请求响应式，短事务
    StatePush,          // 状态上报 - 事件性
    QueryHistoryMedia,  // 历史媒体检索 - 请求响应式
    CheckTime,          // 校时 - 简单请求响应
    SubPub,             // 订阅通知 - 事件性
    Download,           // 下载 - 需要长事务
}
/// 事务 key
pub trait TransactionIdentifier: Send + Sync + HeaderItemExt {
    fn generate_key(&self) -> GlobalResult<String> {
        let key = if self.method_by_cseq()? == Method::Invite {
            format!("INVITE:{}", self.branch()?.value())
        } else {
            format!(
                "{}:{}:{}",
                self.cs_eq()?.value(),
                self.branch()?.value(),
                self.call_id()?.value()
            )
        };
        Ok(key)
    }
}
impl TransactionIdentifier for rsip::Request {}
impl TransactionIdentifier for rsip::Response {}
impl TransactionIdentifier for rsip::SipMessage {}

struct Shard {
    anti_map: HashMap<String, (TransactionRequired, u8, SipMessage)>,
    expire_set: BTreeSet<(Instant, String)>,
}
pub struct TransactionContext {
    pub shard: Arc<RwLock<Shard>>,
}
impl TransactionContext {
    pub fn process_request(
        &self,
        request: &Request,){
        unimplemented!()
    }
    pub fn handle_response(
        &self,
        response: &SipMessage,
    ) -> GlobalResult<usize> {
        unimplemented!()
    }
}
