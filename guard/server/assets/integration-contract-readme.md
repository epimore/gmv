# GMV Guard 第三方集成契约

本目录由 `gmv-guard-server export-integration-contracts <output-directory>` 生成，内容与当前 Guard Server 在线文档同源。

## HTTP

- 基础路径：`/openapi/v1`
- 方法：只使用 GET、POST；写操作使用 POST。
- 鉴权：`GMV-HMAC-SHA256-V1`，字段顺序和请求头见 `openapi.json` 的 `x-gmv-hmac`。
- POST 必须携带 `X-GMV-Request-ID`，该值也必须写入 canonical request；同一应用在 24 小时内以相同请求 ID 和相同内容重试会得到原响应。
- nonce 仍是每次网络请求唯一值；重试时使用新 nonce，但保持相同 `X-GMV-Request-ID`。

## MQTT

- command：`gmv/commands/{integration_id}`
- result：`gmv/command-results/{integration_id}`
- event：`gmv/events/{integration_id}/{event_type}`
- QoS：1，retain：false。
- MQTT V3.1.1 与 V5 使用同一 JSON payload；应用协议版本必须与 Guard 部署级 MQTT runtime 一致。
- broker 的 wildcard subscription 或 ACL 不代表业务授权。Guard 会按每条命令实时检查应用状态、有效期、精确 topic 和 allowed actions。

## 安全说明

契约包不包含 Access Key、Secret、broker 凭据、master key 或环境 endpoint。凭据必须通过受控渠道单独交付，生产环境应启用 TLS 和最小 topic ACL。
