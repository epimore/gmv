# GB28181 SIP Corpus

## Generated Synthetic-Wire Corpus

`generated/` 保存由 `session/tests/sip_corpus.rs` 确定性生成的 44 个 SIP
线格报文及 `manifest.yaml`。这些报文使用固定测试编码、RFC 5737 文档地址、
CRLF 和按实际字节数计算的 `Content-Length`，覆盖正常流程所需的 REGISTER、
OPTIONS、MESSAGE、INVITE/ACK/BYE、INFO、SUBSCRIBE 和 NOTIFY。

该目录的 `source` 固定为 `synthetic-wire`，用于离线回归和质量监控，不是从真实
厂商设备或固件采集的语料，也不改变下方 Required Inventory 的 `Missing` 状态。
`manifest.yaml` 记录场景、方向、方法、预期状态、业务接口、自动化测试和逐文件
SHA-256，可用于检测报文与测试映射漂移。

显式重建和只读校验：

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv/session
GMV_UPDATE_SIP_CORPUS=1 ~/.cargo/bin/cargo test --test sip_corpus
~/.cargo/bin/cargo test --test sip_corpus -- --nocapture
```

正常业务闭环测试：

```bash
~/.cargo/bin/cargo test --lib \
  normal_flow_tests::all_business_http_apis_complete_the_normal_signaling_flow
```

## Standardized Reference Corpus

`reference/` 保存用户提供的 GB/T 28181-2016/2022 标准化 SIP 基准集：

- `gbt28181-2016-2022-baseline.md`：原始 Markdown 基准，SHA-256 固定为
  `2d89bb70302b80f83e1aa9d8956c36d95ddb123c3f0bdf4c5c2519333319a262`。
- `manifest.yaml`：由 `session/tests/sip_corpus.rs` 生成，记录 26 个 TC、108 个 SIP 报文、
  方法分布、方向、Call-ID、CSeq、Content-Type、Content-Length 和逐报文 SHA-256。
- `extracted/`：从 Markdown fenced `sip` block 提取出的 108 个 CRLF 线格报文。

该语料来源为 `standardized-reference`，用于协议质量监控和回归校验；它不是未加工的真实厂商固件抓包，
因此不替代下方 Required Inventory 中仍需采集的真实设备 corpus。

校验和显式重建：

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv/session
GMV_UPDATE_SIP_REFERENCE=1 ~/.cargo/bin/cargo test --test sip_corpus \
  reference_sip_baseline_is_current_and_complete
~/.cargo/bin/cargo test --test sip_corpus -- --nocapture
```

本目录保存 PJSIP 迁移使用的脱敏真实设备报文。2026-06-12 基线调查未找到业务
`register.txt`、pcap 或 GMV 自有 SIP fixture；当前目录只有接收规范，不能视为 G0
真实报文 corpus 已完成。

`gmv_pjsip/tests/sip_flow.rs` 中的合成报文可以作为单元测试基线，但不能替代真实设备样本。

## Required Inventory

| ID | Required sample | Status |
|---|---|---|
| R01 | GB28181-2016 UDP 首次 REGISTER/401/鉴权 REGISTER/200 | Missing |
| R02 | GB28181-2016 TCP 注册、连接复用和断开 | Missing |
| R03 | GB28181-2022 `X-GB-Ver: 3.0` 的 401/403/200 | Missing |
| R04 | 错误密码、错误 realm、过期 nonce 和重放 | Missing |
| R05 | REGISTER refresh 和 Expires=0 unregister | Missing |
| R06 | TCP 重连后 REGISTER 与旧连接关闭竞态 | Missing |
| M01 | Keepalive、DeviceInfo、Catalog、RecordInfo MESSAGE | Missing |
| M02 | PTZ、Preset、Snapshot 和 Alarm MESSAGE | Missing |
| I01 | 实时、回放、下载、对讲 INVITE/ACK/BYE | Missing |
| I02 | CANCEL/200 竞态、设备主动 BYE/CANCEL | Missing |
| S01 | Catalog SUBSCRIBE/NOTIFY/refresh/unsubscribe | Missing |
| O01 | OPTIONS request/response | Missing |

最低厂家范围：海康、大华、宇视、天地伟业；每个样本需记录匿名型号族、固件大版本、
GB28181 版本和 transport。

## Layout

```text
session/tests/fixtures/sip/
  register/
  message/
  invite/
  subscription/
  options/
```

文件名使用：

```text
<vendor>-<gb-version>-<transport>-<scenario>-<sequence>.sip
```

例如：

```text
hikvision-2016-udp-register-auth-01.sip
```

同一流程的 metadata 使用同名 `.yaml`，至少包含：

```yaml
vendor: hikvision
model_family: anonymized
firmware_major: anonymized
gb_version: "2016"
transport: udp
direction: device-to-platform
scenario: register-auth
expected_parse: accept
expected_event: Registered
quirk: null
```

## Sanitization

- 设备、平台、通道 ID 替换为结构合法且全局一致的测试 ID。
- IPv4 使用 RFC 5737 地址，IPv6 使用文档保留地址；端口可保留语义但不保留真实公网映射。
- 域名、Contact、Route、Record-Route 和 User-Agent 中的客户标识必须脱敏。
- 删除密码、HA1、token、cookie、证书和私有扩展中的业务数据。
- Authorization、nonce、cnonce 和 digest response 替换为合成值；需要验证 Digest 时重新
  使用固定测试密码生成自洽报文。
- XML 中姓名、地址、组织、经纬度和业务备注必须替换。
- 保留原始 CRLF、header 顺序、大小写、重复 header、compact header 和 TCP 分段边界。
- 修改 body 后重新计算 `Content-Length`。
- 原始未脱敏文件不得进入仓库、issue、日志或 CI artifact。

## Intake Gate

新增 fixture 必须：

1. 通过敏感信息人工复核。
2. 有 metadata 和明确 expected result。
3. 有自动化测试引用，或在计划中记录尚未自动化的原因。
4. 非标准兼容样本注明厂家/固件范围和最小 quirk。
5. 将 inventory 状态从 `Missing` 更新为 `Collected` 或 `Automated`。
