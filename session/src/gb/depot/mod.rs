/*
### 核心原则：基于“幂等性”和“业务影响”划分
- **幂等操作**：执行一次和执行多次，产生的副作用是相同的。这类请求可以宽松处理。
- **非幂等操作**：执行一次和执行多次，产生的副作用不同，可能造成重复业务或状态混乱。这类请求必须严格防重放。
简单的法则：
- **只读操作、状态维持操作 → 宽松防重放**。目标是保证通信的可靠性。
- **写操作、有副作用的控制操作 → 严格防重放**。目标是保证业务的正确性和系统的安全性。

### 实现层面的最佳实践

#### 1. 策略配置
在你的SIP服务器中，最好能有一个可配置的策略表，将不同的请求类型（Method + URI）映射到“严格”或“宽松”的防重放策略上。

#### 2. “宽松”防重放的实现细节
对于需要宽松处理的请求，防重放缓存的目的不是为了阻止执行，而是为了 **避免不必要的重复处理**，同时保证响应能送达。

- **缓存键**：依然使用 `Call-ID` + `CSeq` + 源地址等。
- **缓存内容**：可以缓存**已生成的响应**。
- **流程**：
  1. 收到请求，检查缓存。
  2. **若命中**：直接从缓存中取出之前的响应，再次发送给请求方。这节省了业务逻辑的处理开销。
  3. **若未命中**：正常处理业务，生成响应，将响应存入缓存（设置一个较短的超时时间，如30秒），然后发送。

#### 3. “严格”防重放的实现细节
对于需要严格处理的请求，必须阻止其业务逻辑被执行第二次。

- **缓存键**：同上。
- **缓存内容**：只需缓存一个“已接收”的标志位。
- **流程**：
  1. 收到请求，检查缓存。
  2. **若命中**：记录安全日志（“检测到重放攻击”），**静默丢弃该请求**。**绝对不要返回4xx错误**，因为错误响应本身也会给攻击者提供信息。
  3. **若未命中**：正常处理业务，生成响应，将“已接收”标志位存入缓存（超时时间可以设置长一些，如10分钟），然后发送。

#### 4. 缓存超时时间
- **宽松策略**：超时短，**覆盖SIP事务超时即可**（如3-30秒）。目的是处理网络重传。
- **严格策略**：超时长，**需要覆盖整个业务会话的生命周期**（如10分钟至1小时）。目的是防止在会话有效期内被重放。
*/
use crate::gb::depot::anti::AntiReplayContext;
use crate::gb::depot::trans::TransactionContext;
use base::exception::GlobalResult;
use base::log::{debug, error};
use base::net::state::{Association, Zip};
use base::tokio::runtime::Handle;
use base::tokio::sync::mpsc::Sender;
use base::tokio::sync::oneshot;
use base::tokio_util::sync::CancellationToken;
use rsip::{Request, Response};
use std::fmt::Display;
use std::pin::Pin;

pub mod anti;
pub mod extract;
/// # GB/T 28181 防重放策略分类
///
/// | 请求类型 | 示例方法 | 防重放策略 | 理由与详细说明 |
/// | :--- | :--- | :--- | :--- |
/// | **查询/状态类** | `MESSAGE` (设备信息、状态、目录查询) | **宽松** | **幂等操作**。多次查询不会改变设备状态。响应丢失会导致客户端无法获取信息，必须允许重试。 |
/// | **注册/保活类** | `REGISTER` | **宽松** | **幂等操作**。核心是刷新在线状态。响应丢失会导致设备离线，必须允许重试以保障连接。 |
/// |  | `MESSAGE` (心跳/Keepalive) | **宽松** | **幂等操作**。同注册，是维持状态的心跳包。 |
/// | **控制类（无状态改变）** | `MESSAGE` (设备配置查询) | **宽松** | **幂等操作**。虽然是控制命令，但查询配置不改变设备状态。 |
/// | **控制类（有状态改变）** | `MESSAGE` (布防、撤防、报警复位、设备配置) | **严格** | **非幂等操作**。执行两次"布防"可能造成重复报警或流程错误。必须防止重复执行。 |
/// | **媒体操作类** | `INVITE` (发起实时视音频点播) | **严格** | **非幂等操作**。重复的INVITE会导致建立多个视频流，耗尽设备、平台和网络资源。 |
/// |  | `MESSAGE` (录像下载) | **严格** | **非幂等操作**。重复请求可能导致发起多次下载任务，造成资源浪费和任务冲突。 |
/// | **对话终止类** | `BYE` (结束视频点播) | **宽松（或特殊处理）** | **幂等操作**。结束一个不存在的会话也无妨。但更佳实践是：检查会话存在性，若存在则结束并响应；若不存在，也回复一个成功的响应，让客户端安心。 |
/// GB/T 28181 防重放策略类型
///
/// 根据请求的幂等性和业务影响，将防重放策略分为两类：
///
/// - **宽松策略**: 用于幂等操作，允许请求重试，确保通信可靠性
/// - **严格策略**: 用于非幂等操作，防止重复执行，确保业务正确性
pub mod trans;

pub struct DepotContext {
    pub anti_ctx: AntiReplayContext,
    pub trans_ctx: TransactionContext,
}
impl DepotContext {
    pub fn init(rt: Handle, cancel_token: CancellationToken, output: Sender<Zip>) -> Self {
        Self {
            anti_ctx: AntiReplayContext::init(),
            trans_ctx: TransactionContext::init(rt, cancel_token, output),
        }
    }
}
pub type TransRx = oneshot::Receiver<GlobalResult<Response>>;
pub type TransTx = oneshot::Sender<GlobalResult<Response>>;
pub type Callback = Box<dyn FnOnce(GlobalResult<Response>) + Sync + Send + 'static>;
pub fn default_log_callback(action_name: impl Display + Send + Sync + 'static) -> Callback {
    let action = format!("{}", action_name);

    Box::new(move |result: GlobalResult<Response>| match result {
        Ok(resp) => {
            let code = resp.status_code.code();
            if (200..300).contains(&code) {
                debug!("{}: 请求成功 (status={})", action, code);
            } else {
                error!("{}: 请求失败 (status={})", action, code);
            }
        }
        Err(e) => {
            error!("{}: 请求异常 - {}", action, e);
        }
    })
}

pub fn default_response_callback(tx: TransTx) -> Callback {
    Box::new(move |result: GlobalResult<Response>| {
        if let Err(_) = tx.send(result) {
            debug!("request point drop");
        }
    })
}

pub enum SipMsg {
    Response(Response),
    Request(Request, Callback),
}
pub struct SipPackage {
    pub sip_msg: SipMsg,
    pub association: Association,
}
impl SipPackage {
    pub fn build_response(response: Response, association: Association) -> Self {
        Self {
            sip_msg: SipMsg::Response(response),
            association,
        }
    }
    pub fn build_request(request: Request, association: Association, callback: Callback) -> Self {
        let sip_pkg = Self {
            sip_msg: SipMsg::Request(request, callback),
            association,
        };
        sip_pkg
    }
}
