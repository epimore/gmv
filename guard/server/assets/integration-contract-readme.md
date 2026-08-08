# GMV Guard 第三方集成契约

本目录由 `gmv-guard-server export-integration-contracts <output-directory>` 生成，内容与当前 Guard Server 在线文档同源。接口和字段以同目录的 `openapi.json`、`asyncapi.json` 为机器可读事实源。

## HTTP

- 基础路径：`/openapi/v1`。
- 方法：只使用 GET、POST；写操作统一使用 POST。
- 鉴权：`GMV-HMAC-SHA256-V1`，字段顺序和请求头见 `openapi.json` 的 `x-gmv-hmac`。
- POST 必须携带 `X-GMV-Request-ID`，该值也必须写入 canonical request。取值为 1～128 个非空白字符，建议使用 UUID/ULID。
- 同一应用在 24 小时内以相同请求 ID 和相同内容重试会得到原响应；相同请求 ID 配合不同内容会被拒绝。
- nonce 仍是每次网络请求唯一值。重试时使用新 nonce，但保持相同 `X-GMV-Request-ID`。

## MQTT 接入前提

现场应为每个第三方应用交付以下非敏感配置和独立凭据：

| 配置 | 说明 |
| --- | --- |
| `integration_id` | 应用唯一标识，必须与消息体和 Topic 绑定关系一致 |
| Broker/TLS/账号 | 由部署方通过受控渠道交付，生产环境应启用 TLS |
| MQTT 版本 | `v3` 表示 MQTT 3.1.1，`v5` 表示 MQTT 5.0；必须与 Guard runtime 一致 |
| `command_topic` | 发布命令的精确 Topic，例如 `gmv/commands/partner-a` |
| `result_topic` | 订阅命令终态的精确 Topic，例如 `gmv/command-results/partner-a` |
| event topics | 按应用 mapping 和 Broker ACL 授权，不应订阅无关应用事件 |
| allowed actions | 应用获授权的 action 清单；Broker ACL 不等于 Guard 业务授权 |

MQTT V3.1.1 与 V5 使用完全相同的 UTF-8 JSON payload。命令和结果推荐 QoS 1，命令必须 `retain=false`。PUBACK 只表示 Broker 收到消息，不表示业务成功；业务终态只以 `result_topic` 的 `state` 为准。

以下 Topic 均为示例。调用方必须使用现场为本应用配置的精确 Topic，不得自行替换 `integration_id` 或使用通配 Topic 发布。

## MQTT 命令统一信封

```json
{
  "integration_id": "partner-a",
  "command_id": "01JMQTTEXAMPLE000000000001",
  "issued_at_ms": 1700000000000,
  "expires_at_ms": 1700000060000,
  "action": "stream.start",
  "target": "device-001",
  "payload": {}
}
```

| 字段 | 必填 | 约束与用途 |
| --- | --- | --- |
| `integration_id` | 是 | 必须等于当前 Topic 绑定的应用标识 |
| `command_id` | 是 | 1～128 个非空白字符；全局唯一；同一业务重试必须复用 |
| `issued_at_ms` | 是 | 调用方签发时的 Unix 毫秒时间戳 |
| `expires_at_ms` | 是 | 不早于 `issued_at_ms`，二者差值不超过 300000 ms；建议 60000 ms |
| `action` | 是 | 必须属于应用 allowed actions，并与 `payload` schema 对应 |
| `target` | 是 | action 的主目标，具体含义见 action 表 |
| `payload` | 是 | JSON 对象；即使 action 无参数也发送 `{}` |

网络超时或暂未收到结果时，可以原样重发同一命令，但必须复用原 `command_id`。不要用新 `command_id` 盲目重试，否则会被视为新业务操作。

## HTTP/MQTT 能力对齐

Guard 对外开放的 HTTP 与 MQTT 是并列协议适配器，均直接调用同一业务控制面，不存在 MQTT 转 HTTP 或 HTTP 转 MQTT。当前 AsyncAPI 注册 62 个 MQTT action，覆盖 OpenAPI 的 58 个开放 path、65 个 method/path 操作（同一 action 可对应多个 HTTP 操作）。完整且可执行的映射位于：

- `openapi.json`：每个 operation 的 `x-gmv-mqtt-action` 或 `x-gmv-mqtt-special`。
- `asyncapi.json`：`x-gmv-http-mqtt-capabilities`、`x-gmv-action-usage` 和 `x-gmv-action-examples`。
- `manifest.json`：交付包能力矩阵，可用于接入方启动检查或代码生成前校验。

协议特例只有明确登记的边界：HTTP `GET /events` 是历史事件轮询，MQTT 使用 event Topic 推送同一类事件；图片、录像文件、播放媒体等二进制或数据面内容不进入 MQTT payload，MQTT 返回受控 access URL/播放 endpoint 后由调用方通过对应数据协议读取。

每个 action 的在线/离线契约均包含 `target` 含义、所需 scope、payload 字段、成功 result 字段、HTTP 等价接口，以及完整 request/success/failure JSON 示例。应用除配置 `allowed_actions` 外，还必须获得 action 对应 scope；任一条件不满足都会 fail-closed。

## MQTT 实时点播完整流程

### 1. 先订阅结果 Topic

在发布命令前订阅应用的精确 `result_topic`，例如：

```text
gmv/command-results/partner-a
```

客户端应以 `command_id` 作为本地命令表的唯一关联键，并允许 QoS 1 重复投递同一结果。

### 2. 发布实时预览命令

发布到 `gmv/commands/partner-a`：

```json
{
  "integration_id": "partner-a",
  "command_id": "01JMQTTLIVE000000000000001",
  "issued_at_ms": 1700000000000,
  "expires_at_ms": 1700000060000,
  "action": "stream.start",
  "target": "device-001",
  "payload": {
    "channel_id": "channel-001",
    "trans_mode": "udp",
    "output_type": "flv",
    "audio_codec": "aac",
    "stream_profile": "main"
  }
}
```

`stream.start` 的 payload：

| 字段 | 必填 | 可选值/范围 | 说明 |
| --- | --- | --- | --- |
| `channel_id` | 是 | 非空字符串 | 要点播的设备通道 ID |
| `device_id` | 否 | 字符串 | 省略时使用 `target`；如同时传，应表示同一设备 |
| `session_node_id` | 否 | 字符串 | 指定 GB28181 Session 节点；省略时由 Guard 调度 |
| `token` | 否 | 字符串 | 媒体订阅 token；省略时 Guard 根据 `command_id` 生成，不应跨点播复用 |
| `trans_mode` | 否 | `udp`、`tcp_active`、`tcp_passive` | 默认 `udp`；`tcp_active` 为 Session 主动连设备，`tcp_passive` 为 Session 等设备连接 |
| `output_type` | 否 | `flv`、`fmp4`、`hls`、`ll_hls` | 省略时使用 Session 默认；`ll_hls` 仅实时预览 |
| `audio_codec` | 否 | `aac` | 当前只支持显式请求 AAC；省略表示不指定转码目标 |
| `stream_profile` | 否 | `main`、`sub` | 默认 `main`；分别表示主码流、辅码流 |

### 3. 处理成功终态

Guard 发布到 `result_topic`：

```json
{
  "schema_version": "v1",
  "integration_id": "partner-a",
  "command_id": "01JMQTTLIVE000000000000001",
  "operation_id": "01JMQTTLIVE000000000000001",
  "action": "stream.start",
  "state": "succeeded",
  "error_code": null,
  "result": {
    "stream_id": "stream-001",
    "device_id": "device-001",
    "channel_id": "channel-001",
    "endpoint": "https://media.example/live/stream-001.flv?token=REDACTED",
    "subscription_id": "subscription-001",
    "requested_stream_profile": "main",
    "effective_stream_profile": "main",
    "state": "running"
  },
  "occurred_at_ms": 1700000001000
}
```

调用方只在 `state=succeeded` 时读取 `result`：

- 将 `result.endpoint` 交给与输出类型匹配的播放器；endpoint 可能包含访问 token，禁止写日志或暴露给无权限终端。
- 保存 `result.stream_id`，停止、取消或超时回收时都需要它。
- `requested_stream_profile` 与 `effective_stream_profile` 可用于确认设备实际采用的码流。

### 4. 停止并释放点播

业务结束后发布 `stream.stop`，`target` 必须使用启动成功返回的 `result.stream_id`：

```json
{
  "integration_id": "partner-a",
  "command_id": "01JMQTTSTOP000000000000001",
  "issued_at_ms": 1700000100000,
  "expires_at_ms": 1700000160000,
  "action": "stream.stop",
  "target": "stream-001",
  "payload": {}
}
```

收到 `state=succeeded` 后可认为停止命令已被 Guard 接受；调用方应清理播放器和本地命令状态。重复停止应复用原停止命令的 `command_id`。

## MQTT 录像回放

回放同样先订阅结果 Topic，再向 command Topic 发布：

```json
{
  "integration_id": "partner-a",
  "command_id": "01JMQTTPLAYBACK00000000001",
  "issued_at_ms": 1700000000000,
  "expires_at_ms": 1700000060000,
  "action": "stream.playback",
  "target": "device-001",
  "payload": {
    "channel_id": "channel-001",
    "start_time_sec": 1699996400,
    "end_time_sec": 1700000000,
    "trans_mode": "tcp_active",
    "output_type": "hls",
    "stream_profile": "main"
  }
}
```

- `start_time_sec`、`end_time_sec` 是 Unix 秒，必须满足 `0 < start_time_sec < end_time_sec`。
- 回放 `output_type` 支持 `flv`、`fmp4`、`hls`，不支持 `ll_hls`、`mp4`。
- 回放只支持 `stream_profile=main`。
- 成功后播放 `result.endpoint`，结束时仍使用 `result.stream_id` 发送 `stream.stop`。

录像下载使用 `action=stream.download`，时间字段相同，`output_type` 可选 `flv`、`fmp4`、`hls`、`mp4`；需要 MP4 时应显式传 `mp4`。成功后读取 `result.endpoint`，完成或取消后执行 `stream.stop`。

## MQTT 云台控制

`device.ptz` 的 `target` 为设备 ID，payload 示例：

```json
{
  "channel_id": "channel-001",
  "leftRight": 1,
  "upDown": 0,
  "inOut": 0,
  "horizonSpeed": 128,
  "verticalSpeed": 0,
  "zoomSpeed": 0
}
```

- `leftRight`：0=不转、1=左、2=右；`upDown`：0=不转、1=上、2=下；`inOut`：0=不变倍、1=缩小、2=放大。
- 水平/垂直轴参与转动时对应速度为 1～255；不参与时传 0。
- 变倍时 `zoomSpeed` 为 1～15；不变倍时传 0。
- 只允许停止 `(0,0,0)`、八方向组合和纯变倍 `(0,0,1|2)`；转动与变倍不能同时发送。
- 停止命令应将三个方向和三个速度都传 0。

## MQTT 常用 action 速查

| action | `target` | payload 必填字段 | 成功 result / 后续动作 |
| --- | --- | --- | --- |
| `stream.start` | `device_id` | `channel_id` | `StreamCommandResult`；播放 endpoint，保存 stream_id |
| `stream.stop` | `stream_id` | 无，传 `{}` | 流停止状态 |
| `stream.playback` | `device_id` | `channel_id`、`start_time_sec`、`end_time_sec` | 播放 endpoint，保存 stream_id |
| `stream.download` | `device_id` | `channel_id`、`start_time_sec`、`end_time_sec` | 读取 endpoint，保存 stream_id |
| `device.broadcast` | `device_id` | `channel_id` | endpoint 为音频输入地址；结束后 stop |
| `device.ptz` | `device_id` | `channel_id`、三个方向、三个速度 | `accepted`、归一化 command、speed、sequence |
| `ai.start` | `stream_id` | `model` | 保存 `task_id`，取消时作为 target |
| `ai.cancel` | `task_id` | 无，传 `{}` | AI 任务终态 |
| `playback.ticket.renew` | 续期事件中的 `token` | `renew` | `renewed`、`revoked`、`expires_at_ms` |

语音广播固定使用 PCMA（G.711 A-law）、8000 Hz、单声道。`broadcast_frame_duration_ms` 为 10～60 ms，默认 20 ms，且 `8000 × 帧时长` 必须能被 1000 整除。完整字段、结果 schema 和示例见 `asyncapi.json`。

上表保留原有 9 个高频 action 作为接入速查，并非 MQTT 能力总表。设备/通道/资源、截图、录像检索、云录像、广播任务、流输出、回放控制、节点/租约/运行状态等其余 action 应从 `asyncapi.json` 的 `x-gmv-action-usage` 读取，避免客户端维护会漂移的手写清单。

## 失败、超时和重复投递

失败终态示例：

```json
{
  "schema_version": "v1",
  "integration_id": "partner-a",
  "command_id": "01JMQTTPLAYBACK00000000001",
  "operation_id": "01JMQTTPLAYBACK00000000001",
  "action": "stream.playback",
  "state": "failed",
  "error_code": "invalid_command",
  "result": null,
  "occurred_at_ms": 1700000001000
}
```

- `state=failed` 时不得尝试使用 `result`，应按稳定 `error_code` 收敛业务状态并保留 `command_id` 供排障。
- QoS 1 允许命令或结果重复投递。生产者用 `command_id` 保证业务幂等，消费者也必须按 `command_id` 幂等更新本地状态。
- 如果在本地等待窗口内未收到结果，先检查订阅是否早于发布、Topic/ACL、应用状态和命令有效期，再用相同 `command_id` 重发。
- 不得把 Broker PUBACK、发布 API 返回成功或消息进入客户端发送队列当作业务成功。

## 安全说明

契约包不包含 Access Key、Secret、Broker 凭据、master key 或环境 endpoint。凭据必须通过受控渠道单独交付；生产环境应启用 TLS、独立账号和最小 Topic ACL。日志和错误中不得输出密码、完整 endpoint、媒体 token 或播放票据。
