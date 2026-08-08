# GMV 打包部署交付文档

本文面向第三方交付、部署和运维人员，覆盖 GMV Guard 控制面、GB28181 Session 节点、Stream 流媒体节点的打包、离线交付、Nginx TLS 代理、安全边界、静态依赖和跨平台构建。

## 1. 交付范围

### 1.1 组件说明

| 组件 | Crate | 二进制 | 说明 |
| --- | --- | --- | --- |
| Guard Server | `gmv-guard-server` | `guard` | 控制面、Web UI、认证、节点注册、路由、事件、业务 API |
| GB28181 Session | `gmv-session-gb28181` | `gmv-session-gb28181` | GB28181 SIP 信令、设备/通道管理、抓拍、录像下载、向 Guard 注册 |
| Stream | `gmv-stream` | `gmv-stream` | RTP/FLV/HLS 等流媒体接入与播放输出、向 Guard 注册 |
| Guard UI | `guard/ui` | `dist/` 或嵌入 `guard` | Vue 前端控制台，包含 `guard/player`、`hls.js`、`mpegts.js` |
| PJSIP | sibling `../gmv_pjsip` + `third_party/pjproject-2.17/dist` | 静态库 | GB28181 Session 的 SIP 原生依赖 |
| FFmpeg | `third_party/ffmpeg-6.1/dist` | 静态库 | Stream 的媒体封装/转码基础依赖 |

### 1.2 典型运行拓扑

单机部署：

```text
Browser
  -> Nginx(可选，TLS/域名)
  -> guard
  -> session-gb28181
  -> stream
```

多机部署：

```text
Browser -> Nginx -> guard
                    |-- session-gb28181 节点 1..N
                    |-- stream 节点 1..N
```

Guard 负责控制面和 UI；Session、Stream 作为业务节点向 Guard 的 gRPC 地址注册。

## 2. 目录和依赖约定

### 2.1 源码目录

推荐源码目录保持如下 sibling 关系：

```text
/home/ubuntu20/code/rs/mv/github/epimore/
  gmv/
  gmv_pjsip/
  pigs/
```

`session/gb28181` 依赖 sibling `gmv_pjsip` 和 `pigs/base_db`。如果改变目录结构，需要同步修改 Cargo path 依赖。

### 2.2 构建机要求

构建机需要：

```text
Rust/Cargo
Node.js
pnpm
clang/gcc/make/pkg-config
Docker 或 cross，跨平台构建时需要
curl/tar
```

运行设备不需要 Node.js、pnpm、Rust，也不需要访问公网。

### 2.3 离线交付原则

- 构建阶段完成所有依赖下载、编译和前端打包。
- 运行设备只接收二进制、配置文件、可选静态前端产物、证书、systemd/Nginx 配置。
- 不在运行设备上执行 `pnpm install`、`cargo build`、PJSIP/FFmpeg 编译。
- 不把源码目录、`node_modules`、`target`、`third_party` 源码包作为运行期依赖交付。

## 3. 构建前准备

进入仓库根目录：

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv
```

确认工作区：

```bash
git status --short
```

建议构建前执行：

```bash
~/.cargo/bin/cargo check -p gmv-guard-server
~/.cargo/bin/cargo check -p gmv-session-gb28181
~/.cargo/bin/cargo check -p gmv-stream
pnpm -C guard/ui typecheck
```

## 4. 前端打包

Guard UI 构建命令：

```bash
pnpm -C guard/ui install --frozen-lockfile
pnpm -C guard/ui build
```

产物：

```text
guard/ui/dist/index.html
guard/ui/dist/assets/*
```

说明：

- `guard/ui` 会通过别名引入 `guard/player/src`，不需要单独部署 `guard/player/dist`。
- `hls.js`、`mpegts.js` 已作为 `guard/ui` 显式依赖，构建后会进入本地 `dist/assets`。
- 运行环境无公网时，不需要额外提供 CDN、播放器 JS 或静态库。
- 默认不生成 sourcemap，交付时不要开启 `build.sourcemap`。

如果需要独立验证播放器 demo，可额外执行：

```bash
pnpm -C guard/player build
```

Guard 正式部署不依赖 `guard/player/dist`。

## 5. Guard 打包方式

Guard 支持两种交付方式：分开部署和集成部署。

### 5.1 分开部署

适用场景：

- 前端由 Nginx 单独托管；
- 需要替换 UI 但不想重编译后端；
- 需要保留清晰的静态文件目录。

构建：

```bash
pnpm -C guard/ui install --frozen-lockfile
pnpm -C guard/ui build
~/.cargo/bin/cargo build -p gmv-guard-server --release
```

交付物：

```text
target/release/gmv-guard-server
guard/server/config.yml
guard/ui/dist/
```

推荐安装目录：

```text
/opt/gmv/
  bin/
    guard
  config/
    guard.yml
  guard-ui/
    dist/
      index.html
      assets/
  data/
  logs/
```

由 Guard 自己托管前端时：

```yaml
guard:
  http:
    bind_addr: 0.0.0.0:8080
    origins:
      - http://127.0.0.1:8080
      - http://设备IP:8080
    ui_dist_dir: /opt/gmv/guard-ui/dist
    tls:
      enabled: false
    # Nginx 已实际协商 h2 后才设为 true；否则多画面上限保持 6。
    media_https_http2_verified: true
```

启动：

```bash
/opt/gmv/bin/guard start -c /opt/gmv/config/guard.yml
```

访问：

```text
http://设备IP:8080/
```

### 5.2 集成部署

适用场景：

- 嵌入式或封闭内网环境；
- 希望交付文件最少；
- 不希望运行设备额外携带前端静态目录。

构建顺序必须是先前端、后后端：

```bash
pnpm -C guard/ui install --frozen-lockfile
pnpm -C guard/ui build
~/.cargo/bin/cargo build -p gmv-guard-server --release --features embed-ui
```

交付物：

```text
target/release/gmv-guard-server
guard/server/config.yml
```

集成部署下，`guard.http.ui_dist_dir` 会被忽略，但可以保留在配置文件中。

推荐配置：

```yaml
guard:
  http:
    bind_addr: 127.0.0.1:8080
    origins:
      - https://your-domain.example.com
    ui_dist_dir: guard/ui/dist
    tls:
      enabled: false
```

启动：

```bash
/opt/gmv/bin/guard start -c /opt/gmv/config/guard.yml
```

### 5.3 Guard 常用命令

启动：

```bash
guard start -c /path/to/guard.yml
```

重置管理员密码：

```bash
guard reset-admin-password -c /path/to/guard.yml -u admin '新密码'
```

配置加密/解密：

```bash
guard encrypt <plaintext>
guard decrypt <ciphertext>
```

### 5.4 指定数据库后端构建

Guard 默认构建同时支持 MySQL 和 SQLite bundled：

```bash
~/.cargo/bin/cargo build -p gmv-guard-server --release
```

按现场数据库裁剪二进制时，可以选择单后端构建：

```bash
# MySQL-only
~/.cargo/bin/cargo build -p gmv-guard-server --release --no-default-features --features db-mysql

# SQLite-only bundled
~/.cargo/bin/cargo build -p gmv-guard-server --release --no-default-features --features db-sqlite

# 集成 UI + 全数据库支持
~/.cargo/bin/cargo build -p gmv-guard-server --release --features embed-ui

# 集成 UI + SQLite-only bundled
~/.cargo/bin/cargo build -p gmv-guard-server --release --no-default-features --features "embed-ui,db-sqlite"
```

选择原则：

- 通用交付包使用默认全支持，现场可在 `guard.database.backend` 中选择 `sqlite` 或 `mysql`。
- 嵌入式单机且确认只使用 SQLite 时，可使用 `db-sqlite` 减小体积。
- 已有 MySQL 或多节点集中数据库部署时，可使用 `db-mysql`。
- 单后端二进制不能运行另一种数据库配置；例如 `db-sqlite` 构建遇到 `backend: mysql` 会启动失败并提示未启用该后端。
- `--no-default-features` 后必须显式指定 `db-mysql` 或 `db-sqlite`；只指定 `embed-ui` 会编译失败。

当前构建机实测 release 体积如下。该数值用于估算交付包大小，实际会随目标平台、profile、strip、依赖版本变化。

| 组件 | 构建特性 | 二进制 | 大小 bytes | 大小 MiB | 相对全支持 |
| --- | --- | --- | ---: | ---: | ---: |
| Guard | `db-all` | `target/release/gmv-guard-server` | 21,014,272 | 20.04 | 基准 |
| Guard | `db-mysql` | `target/release/gmv-guard-server` | 17,064,184 | 16.27 | -3.77 MiB |
| Guard | `db-sqlite` | `target/release/gmv-guard-server` | 17,871,048 | 17.04 | -3.00 MiB |
| Guard | `db-all,embed-ui` | `target/release/gmv-guard-server` | 24,134,712 | 23.02 | +2.98 MiB |

## 6. Nginx TLS 和域名代理

### 6.1 集成部署也可以使用 Nginx

集成部署只是把 UI 放进 `guard` 二进制，不影响 Nginx 代理。推荐拓扑：

```text
Browser
  -> https://your-domain.example.com
  -> Nginx TLS
  -> http://127.0.0.1:8080
  -> guard(内嵌 UI + API)
```

Guard 配置：

```yaml
guard:
  http:
    bind_addr: 127.0.0.1:8080
    origins:
      - https://your-domain.example.com
    tls:
      enabled: false
```

Nginx 配置：

```nginx
server {
    listen 80;
    server_name your-domain.example.com;
    return 301 https://$host$request_uri;
}

server {
    listen 443 ssl http2;
    server_name your-domain.example.com;

    ssl_certificate     /etc/nginx/certs/your-domain.crt;
    ssl_certificate_key /etc/nginx/certs/your-domain.key;

    client_max_body_size 64m;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }

    location /metrics {
        allow 127.0.0.1;
        deny all;
        proxy_pass http://127.0.0.1:8080/metrics;
    }

    # 如果 Stream 也通过同一个域名输出播放地址，并且 stream.server.proxy_addr
    # 配置为 https://your-domain.example.com/s1，则需要代理 /s1/。
    location /s1/ {
        proxy_pass http://127.0.0.1:28570/s1/;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
}
```

`/s1/` 的 access log 不应记录完整 `$request` 或 `$request_uri`，因为播放 token 位于 query string；请在 `http` 块使用只记录 `$uri` 的专用 `log_format`，再为该 location 配置对应 `access_log`。

### 6.2 分开部署使用 Nginx 托管 UI

Guard 配置：

```yaml
guard:
  http:
    bind_addr: 127.0.0.1:8080
    origins:
      - https://your-domain.example.com
    ui_dist_dir: /opt/gmv/guard-ui/dist
    tls:
      enabled: false
```

Nginx 配置：

```nginx
server {
    listen 443 ssl http2;
    server_name your-domain.example.com;

    ssl_certificate     /etc/nginx/certs/your-domain.crt;
    ssl_certificate_key /etc/nginx/certs/your-domain.key;

    root /opt/gmv/guard-ui/dist;
    index index.html;

    location / {
        try_files $uri $uri/ /index.html;
    }

    location /api/ {
        proxy_pass http://127.0.0.1:8080/api/;
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
    }

    location /health/ {
        proxy_pass http://127.0.0.1:8080/health/;
    }

    location /metrics {
        allow 127.0.0.1;
        deny all;
        proxy_pass http://127.0.0.1:8080/metrics;
    }

    location /s1/ {
        proxy_pass http://127.0.0.1:28570/s1/;
        proxy_http_version 1.1;
        proxy_set_header Connection "";
        proxy_set_header Host $host;
        proxy_set_header X-Forwarded-Proto https;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_buffering off;
        proxy_cache off;
        proxy_read_timeout 3600s;
        proxy_send_timeout 3600s;
    }
}
```

不要让同一个 `location /` 同时 `root` 静态目录又 `proxy_pass` 到 Guard。

## 7. GB28181 Session 打包

### 7.1 PJSIP 静态依赖

`gmv-session-gb28181` 依赖 sibling `../gmv_pjsip`，同时需要 PJSIP 静态库。仓库默认通过 `.cargo/config.toml` 指向：

```text
third_party/pjproject-2.17/dist/include
third_party/pjproject-2.17/dist/lib
```

首次构建前执行：

```bash
./session/gb28181/build_pjsip_bootstrap.sh
```

脚本会下载并构建 PJSIP 2.17，生成：

```text
third_party/pjproject-2.17/dist/include
third_party/pjproject-2.17/dist/lib/*.a
```

如需强制重建：

```bash
FORCE_REBUILD=1 ./session/gb28181/build_pjsip_bootstrap.sh
```

### 7.2 构建 Session

```bash
~/.cargo/bin/cargo build -p gmv-session-gb28181 --release
```

默认构建同时支持 MySQL 和 SQLite bundled。按现场数据库裁剪二进制时：

```bash
# MySQL-only
~/.cargo/bin/cargo build -p gmv-session-gb28181 --release --no-default-features --features db-mysql

# SQLite-only bundled
~/.cargo/bin/cargo build -p gmv-session-gb28181 --release --no-default-features --features db-sqlite
```

交付物：

```text
target/release/gmv-session-gb28181
session/gb28181/config.yml
```

运行设备不需要 PJSIP `dist`，只需要已链接好的二进制。若目标平台要求动态系统库，应通过 `ldd` 检查。

当前构建机实测 release 体积：

| 组件 | 构建特性 | 二进制 | 大小 bytes | 大小 MiB | 相对全支持 |
| --- | --- | --- | ---: | ---: | ---: |
| Session | `db-all` | `target/release/gmv-session-gb28181` | 23,002,968 | 21.94 | 基准 |
| Session | `db-mysql` | `target/release/gmv-session-gb28181` | 19,829,800 | 18.91 | -3.03 MiB |
| Session | `db-sqlite` | `target/release/gmv-session-gb28181` | 20,692,176 | 19.73 | -2.20 MiB |

### 7.3 Session 配置要点

示例：

```yaml
guard:
  endpoint: http://127.0.0.1:18080

server:
  grpc:
    addr: 127.0.0.1:19081
    tls:
      enabled: false
  session:
    domain: "3402000000"
    domain_id: "34020000002000000001"
    lan_ip: 192.168.1.10
    wan_ip: 192.168.1.10
    lan_port: 35600
    wan_port: 35600
  pics:
    push_url: http://192.168.1.10:28567/edge/upload/picture
    storage_path: /opt/gmv/session/pics/raw
  download:
    storage_path: /opt/gmv/session/videos/down
```

重点：

- `guard.endpoint` 是 Guard 内部 gRPC 地址，对应 Guard 配置 `guard.grpc.bind_addr`。
- `server.session.domain_id` 是 Session 节点 ID，必须在 Guard `allowed_nodes` 中放行，或关闭 Guard 节点白名单检查。
- `lan_ip` 不要写 `0.0.0.0` 或 `127.0.0.1`，必须写设备可达网卡 IP。
- `lan_port/wan_port` 是 GB28181 SIP 端口，需要防火墙放行 TCP/UDP。
- SQLite 和 MySQL 二选一；嵌入式单机推荐 SQLite，集群或已有数据库推荐 MySQL。

### 7.4 启动 Session

```bash
/opt/gmv/bin/gmv-session-gb28181 start -c /opt/gmv/config/session-gb28181.yml
```

如启动参数由基础 daemon 解析，保持和 Guard/Stream 一致使用 `start -c <config>`。

## 8. Stream 打包

### 8.1 FFmpeg 静态依赖

`gmv-stream` 通过 `rsmpeg` 链接系统 FFmpeg，仓库默认 `.cargo/config.toml` 指向：

```text
third_party/ffmpeg-6.1/dist/include
third_party/ffmpeg-6.1/dist/lib
```

首次构建前执行：

```bash
./stream/build_ffmpeg_min_bootstrap.sh
source ./stream/env_ffmpeg.sh
```

生成：

```text
third_party/ffmpeg-6.1/dist/include
third_party/ffmpeg-6.1/dist/lib/*.a
third_party/ffmpeg-6.1/dist/lib/pkgconfig
```

如果使用自定义 FFmpeg 目录：

```bash
source ./stream/env_ffmpeg.sh /abs/path/to/ffmpeg-dist
```

### 8.2 构建 Stream

```bash
source ./stream/env_ffmpeg.sh
~/.cargo/bin/cargo build -p gmv-stream --release
```

交付物：

```text
target/release/gmv-stream
stream/config.yml
```

### 8.3 Stream 配置要点

示例：

```yaml
guard:
  endpoint: http://127.0.0.1:18080

server:
  name: s1
  host: 192.168.1.10
  rtp_port: 28568
  rtcp_port: 18569
  http_port: 28570
  grpc:
    addr: 127.0.0.1:19082
    tls:
      enabled: false
  http:
    tls:
      enabled: false
  proxy_addr: https://your-domain.example.com/s1
```

重点：

- `server.name` 是 Stream 节点 ID，必须在 Guard `allowed_nodes` 中放行，或关闭 Guard 节点白名单检查。
- `server.host` 是 Session/Guard 可达的本节点地址。
- `rtp_port` 需要对 Session 节点可达。
- `http_port` 是播放输出端口；若通过 Nginx 对外统一代理，`proxy_addr` 应写外部访问前缀。
- `server.http.tls.enabled=false` 时，TLS 由 Nginx 负责。

### 8.4 启动 Stream

```bash
/opt/gmv/bin/gmv-stream start -c /opt/gmv/config/stream.yml
```

## 9. Guard 节点白名单

Guard 配置中：

```yaml
guard:
  registry:
    node_check_enabled: true
    allowed_nodes:
      - id: "34020000002000000001"
        kind: SESSION-GB28181
      - id: "s1"
        kind: STREAM
```

必须保证：

- Session 的 `server.session.domain_id` 与 `allowed_nodes[].id` 一致；
- Stream 的 `server.name` 与 `allowed_nodes[].id` 一致；
- `kind` 使用 `SESSION-GB28181` 或 `STREAM`。

## 10. 静态打包和动态库检查

### 10.1 Linux GNU

GNU 目标通常仍依赖系统 libc 等动态库。构建：

```bash
~/.cargo/bin/cargo build --release -p gmv-guard-server --features embed-ui
~/.cargo/bin/cargo build --release -p gmv-session-gb28181
source ./stream/env_ffmpeg.sh
~/.cargo/bin/cargo build --release -p gmv-stream
```

检查动态依赖：

```bash
ldd target/release/gmv-guard-server
ldd target/release/gmv-session-gb28181
ldd target/release/gmv-stream
```

### 10.2 Linux MUSL 静态目标

MUSL 更适合嵌入式/离线设备，目标示例：

```bash
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl
rustup target add armv7-unknown-linux-musleabihf
```

使用 cross：

```bash
cross build --release --target x86_64-unknown-linux-musl -p gmv-guard-server --features embed-ui
cross build --release --target x86_64-unknown-linux-musl -p gmv-session-gb28181
cross build --release --target x86_64-unknown-linux-musl -p gmv-stream
```

检查：

```bash
file target/x86_64-unknown-linux-musl/release/gmv-guard-server
ldd target/x86_64-unknown-linux-musl/release/gmv-guard-server || true
```

如果显示 `not a dynamic executable`，说明没有常规动态库依赖。

注意：

- PJSIP 和 FFmpeg 也必须为目标平台构建静态库。
- 当前 `build_pjsip_bootstrap.sh` 和 `build_ffmpeg_min_bootstrap.sh` 主要面向本机构建。
- 跨平台静态构建建议使用 `Cross.toml` 中对应的 `static-cross-*` 镜像，并确保镜像内含目标平台 PJSIP/FFmpeg 静态库。

## 11. 跨平台打包

### 11.1 支持目标

`Cross.toml` 和 `makefile` 中列出了目标，包括：

```text
x86_64-unknown-linux-gnu
aarch64-unknown-linux-gnu
armv7-unknown-linux-gnueabihf
i686-unknown-linux-gnu
x86_64-unknown-linux-musl
aarch64-unknown-linux-musl
armv7-unknown-linux-musleabihf
aarch64-linux-android
armv7-linux-androideabi
x86_64-linux-android
i686-linux-android
x86_64-pc-windows-gnu
i686-pc-windows-gnu
aarch64-pc-windows-gnullvm
```

### 11.2 构建自定义 cross 镜像

基础 Dockerfile：

```text
Dockerfile.base
```

示例：

```bash
docker build \
  --build-arg TARGET=x86_64-unknown-linux-gnu \
  -t static-cross-x86_64-unknown-linux-gnu:latest \
  -f Dockerfile.base .
```

为其他目标重复构建，并确保镜像名与 `Cross.toml` 一致。

### 11.3 单目标 cross 构建

```bash
cross build --release --target x86_64-unknown-linux-gnu
```

或使用 makefile：

```bash
make build TARGET=x86_64-unknown-linux-gnu
```

只构建指定包：

```bash
cross build --release --target x86_64-unknown-linux-gnu -p gmv-guard-server --features embed-ui
cross build --release --target x86_64-unknown-linux-gnu -p gmv-session-gb28181
cross build --release --target x86_64-unknown-linux-gnu -p gmv-stream
```

### 11.4 批量构建

```bash
make linux
make musl
make windows
make all
```

注意：

- Windows 仅说明 GNU toolchain，不使用 MSVC。
- Apple/iOS/BSD 目标是否可用取决于本地 cross 镜像和 native 依赖是否完备。
- Stream 的 FFmpeg、Session 的 PJSIP 是跨平台构建成败的关键。

### 11.5 压缩产物

仓库提供：

```bash
./scripts/compress.sh x86_64-unknown-linux-gnu
./scripts/compress.sh all
```

或：

```bash
make compress TARGET=x86_64-unknown-linux-gnu
make compress-all
```

说明：

- 压缩使用 UPX。
- Apple/iOS 等目标会跳过。
- 压缩前会在 `target/backups` 保存原始二进制。
- 生产交付前建议在目标设备上验证 UPX 压缩后的二进制可正常启动。

## 12. 推荐交付目录

### 12.1 单机集成部署

```text
gmv-delivery/
  bin/
    guard
    gmv-session-gb28181
    gmv-stream
  config/
    guard.yml
    session-gb28181.yml
    stream.yml
  systemd/
    gmv-guard.service
    gmv-session-gb28181.service
    gmv-stream.service
  nginx/
    gmv.conf
  certs/
    your-domain.crt
    your-domain.key
  data/
  logs/
  pics/
  videos/
  VERSION
  SHA256SUMS
  PACKAGING_DEPLOYMENT.md
```

### 12.2 分开部署

```text
gmv-delivery/
  bin/
    guard
    gmv-session-gb28181
    gmv-stream
  guard-ui/
    dist/
      index.html
      assets/
  config/
  systemd/
  nginx/
  certs/
  data/
  logs/
  VERSION
  SHA256SUMS
```

### 12.3 生成校验文件

```bash
cd gmv-delivery
find bin config guard-ui -type f -print0 2>/dev/null | sort -z | xargs -0 sha256sum > SHA256SUMS
```

## 13. systemd 示例

### 13.1 Guard

```ini
[Unit]
Description=GMV Guard Server
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/gmv
ExecStart=/opt/gmv/bin/guard start -c /opt/gmv/config/guard.yml
Restart=always
RestartSec=3
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
```

### 13.2 Session

```ini
[Unit]
Description=GMV GB28181 Session
After=network-online.target gmv-guard.service
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/gmv
ExecStart=/opt/gmv/bin/gmv-session-gb28181 start -c /opt/gmv/config/session-gb28181.yml
Restart=always
RestartSec=3
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
```

### 13.3 Stream

```ini
[Unit]
Description=GMV Stream Server
After=network-online.target gmv-guard.service
Wants=network-online.target

[Service]
Type=simple
WorkingDirectory=/opt/gmv
ExecStart=/opt/gmv/bin/gmv-stream start -c /opt/gmv/config/stream.yml
Restart=always
RestartSec=3
LimitNOFILE=1048576

[Install]
WantedBy=multi-user.target
```

启用：

```bash
sudo cp systemd/*.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable --now gmv-guard
sudo systemctl enable --now gmv-session-gb28181
sudo systemctl enable --now gmv-stream
```

查看日志：

```bash
journalctl -u gmv-guard -f
journalctl -u gmv-session-gb28181 -f
journalctl -u gmv-stream -f
```

## 14. 数据库和持久化目录

### 14.1 Guard 数据库

干净预览版采用全新安装数据库基线。交付包中的 `data/` 必须为空，不得携带开发、测试或历史环境的 `guard.db`。Guard 初始化后只应出现：

- `_base_db_migrations`
- `guard_user`
- `guard_outbox`
- `guard_command`
- `guard_integration`
- `guard_integration_credential`
- `guard_integration_http`
- `guard_integration_mqtt`
- `guard_integration_mapping`
- `guard_integration_audit`
- `guard_integration_delivery`

节点、租约、运行路由和事件当前由 Guard 内存状态承载，不创建同名数据库表。该预览版不承诺旧 Guard 数据库原地升级，启动时不会自动删除旧表。

嵌入式或单机部署推荐 SQLite：

```yaml
guard:
  database:
    backend: sqlite
    auto_migrate: true
    sqlite:
      path: /opt/gmv/data/guard.db
```

MySQL 部署示例：

```yaml
guard:
  database:
    backend: mysql
    auto_migrate: true
    mysql:
      host: 127.0.0.1
      port: 3306
      database: gmv
      username: gmv
      pass_crypto_enable: false
      pass: ""
      ssl_mode: preferred
```

Guard migration 已嵌入二进制，并在 `auto_migrate=true` 时按 `_base_db_migrations` 自动执行。不得恢复或执行遗留预览库清理脚本：当前 `guard_integration*` 已是正式业务表，旧脚本可能造成第三方应用、凭据、mapping 和投递状态不可恢复丢失。历史库需要清理时必须先备份，并针对现场 schema 单独评审 SQL；发布包不提供通用删表脚本。

### 14.2 Session 数据库

Session 可使用 SQLite 或 MySQL。SQLite 示例：

```yaml
db:
  backend: sqlite
  sqlite:
    path: /opt/gmv/data/session-gb28181.db
    max_connections: 16
```

MySQL 示例：

```yaml
db:
  backend: mysql
  mysql:
    host_or_ip: 127.0.0.1
    port: 3306
    db_name: gmv
    user: gmv
    pass_crypto_enable: false
    pass: ""
```

Session 初始化直接执行模块自有 schema SQL，不依赖 `_base_db_migrations` 账本表。

数据库配置必须和二进制构建特性匹配：

- 默认 `db-all` 二进制支持 `backend: sqlite` 和 `backend: mysql`。
- `db-sqlite` 二进制只能使用 `backend: sqlite`。
- `db-mysql` 二进制只能使用 `backend: mysql`。
- 配错后端时，Guard 和 Session 会在启动阶段返回明确配置错误，不会自动降级或切换数据库。

### 14.3 必须持久化的目录

```text
/opt/gmv/config
/opt/gmv/data
/opt/gmv/logs
/opt/gmv/pics
/opt/gmv/videos
```

升级时不要覆盖这些目录。

## 15. 端口清单

| 组件 | 配置项 | 默认/示例 | 说明 |
| --- | --- | --- | --- |
| Guard HTTP/UI/API | `guard.http.bind_addr` | `127.0.0.1:8080` | Web UI、API、health、metrics |
| Guard gRPC | `guard.grpc.bind_addr` | `127.0.0.1:18080` | 节点注册、控制面 RPC |
| Session HTTP | `http.port` | `28567` | 抓拍上传等 Session HTTP |
| Session gRPC | `server.grpc.addr` | `127.0.0.1:19081` | Session 控制 RPC |
| Session SIP | `server.session.lan_port` | `35600` | GB28181 SIP TCP/UDP |
| Stream HTTP | `server.http_port` | `28570` | 播放输出 HTTP |
| Stream gRPC | `server.grpc.addr` | `127.0.0.1:19082` | Stream 控制 RPC |
| Stream RTP | `server.rtp_port` | `28568` | RTP 媒体接收 |
| Stream RTCP | `server.rtcp_port` | `18569` | 预留/配置项 |
| Nginx TLS | `listen 443` | `443` | 外部 HTTPS |

防火墙建议：

- 对外只开放 Nginx `80/443` 和必要的 SIP/RTP 端口。
- Guard HTTP、Guard gRPC、Session/Stream gRPC 优先绑定内网或本机地址。
- `/metrics` 仅对本机或监控网段开放。

## 16. 安全注意事项

### 16.1 认证边界

业务 API `/api/v2/*` 需要 UI session、角色和 CSRF 校验：

- Viewer：读取节点、设备、流、事件等；
- Operator：预览、回放、下载、对讲、云台、停流、AI 操作等；
- Admin：用户管理等。

未鉴权的运维接口：

```text
/health/live
/health/ready
/metrics
```

建议通过 Nginx 或防火墙限制 `/metrics`，必要时限制 `/health/ready`。

### 16.2 静态文件边界

分开部署时，`guard.http.ui_dist_dir` 只应指向前端构建产物目录：

```text
/opt/gmv/guard-ui/dist
```

不要指向源码目录、配置目录、证书目录或包含敏感文件的目录。

### 16.3 TLS

推荐由 Nginx 统一处理外部 TLS：

```text
Browser -> HTTPS -> Nginx -> HTTP 127.0.0.1 -> Guard/Stream
```

内部 gRPC TLS 可按生产安全要求逐步启用。启用后需要同步配置证书路径，并确认节点注册上报的 scheme 与 Guard 侧调用一致。

### 16.4 密码和密钥

- 不在交付文档、日志、命令输出中泄露真实密码。
- 首次部署完成后立即修改管理员密码。
- 已初始化用户后，建议移除或清空 bootstrap 初始密码。
- 证书私钥权限建议设置为 `600`。

## 17. 启动顺序和验收

### 17.1 启动顺序

单机建议：

```text
1. Nginx 证书和配置就绪
2. Guard 启动
3. Stream 启动并向 Guard 注册
4. Session 启动并向 Guard 注册
5. 浏览器登录 Guard UI 验证节点和设备
```

### 17.2 命令验收

Guard：

```bash
curl http://127.0.0.1:8080/health/live
curl http://127.0.0.1:8080/health/ready
```

Nginx TLS：

```bash
curl -k https://your-domain.example.com/health/live
```

节点进程：

```bash
systemctl status gmv-guard
systemctl status gmv-session-gb28181
systemctl status gmv-stream
```

端口：

```bash
ss -lntup | grep -E '8080|18080|19081|19082|28567|28568|28570|35600'
```

动态库：

```bash
ldd /opt/gmv/bin/guard
ldd /opt/gmv/bin/gmv-session-gb28181
ldd /opt/gmv/bin/gmv-stream
```

MUSL 静态二进制可用：

```bash
ldd /opt/gmv/bin/guard || true
```

如果输出 `not a dynamic executable`，说明无需常规动态库。

### 17.3 UI 验收

浏览器访问：

```text
https://your-domain.example.com/
```

检查：

- 登录页可打开；
- 使用管理员账号登录；
- Dashboard 正常；
- 节点列表能看到 Session/Stream；
- GB28181 节点配置、设备列表可访问；
- 预览/回放能返回播放地址；
- 浏览器开发者工具 Network 不应请求公网 CDN。

## 18. 交付包制作示例

### 18.1 集成部署交付包

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv

rm -rf /tmp/gmv-delivery
mkdir -p /tmp/gmv-delivery/{bin,config,contracts,systemd,nginx,certs,data,logs,pics,videos}

cp target/release/gmv-guard-server /tmp/gmv-delivery/bin/guard
cp target/release/gmv-session-gb28181 /tmp/gmv-delivery/bin/
cp target/release/gmv-stream /tmp/gmv-delivery/bin/

cp guard/server/config.yml /tmp/gmv-delivery/config/guard.yml
cp session/gb28181/config.yml /tmp/gmv-delivery/config/session-gb28181.yml
cp stream/config.yml /tmp/gmv-delivery/config/stream.yml
cp PACKAGING_DEPLOYMENT.md /tmp/gmv-delivery/
/tmp/gmv-delivery/bin/guard export-integration-contracts /tmp/gmv-delivery/contracts

git rev-parse HEAD > /tmp/gmv-delivery/VERSION
date -u +"build_time_utc=%Y-%m-%dT%H:%M:%SZ" >> /tmp/gmv-delivery/VERSION

cd /tmp/gmv-delivery
find bin config contracts -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
tar -czf ../gmv-delivery-integrated.tar.gz .
```

### 18.2 分开部署交付包

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv

rm -rf /tmp/gmv-delivery
mkdir -p /tmp/gmv-delivery/{bin,config,contracts,systemd,nginx,certs,data,logs,pics,videos,guard-ui}

cp target/release/gmv-guard-server /tmp/gmv-delivery/bin/guard
cp target/release/gmv-session-gb28181 /tmp/gmv-delivery/bin/
cp target/release/gmv-stream /tmp/gmv-delivery/bin/
cp -a guard/ui/dist /tmp/gmv-delivery/guard-ui/

cp guard/server/config.yml /tmp/gmv-delivery/config/guard.yml
cp session/gb28181/config.yml /tmp/gmv-delivery/config/session-gb28181.yml
cp stream/config.yml /tmp/gmv-delivery/config/stream.yml
cp PACKAGING_DEPLOYMENT.md /tmp/gmv-delivery/
/tmp/gmv-delivery/bin/guard export-integration-contracts /tmp/gmv-delivery/contracts

git rev-parse HEAD > /tmp/gmv-delivery/VERSION
date -u +"build_time_utc=%Y-%m-%dT%H:%M:%SZ" >> /tmp/gmv-delivery/VERSION

cd /tmp/gmv-delivery
find bin config contracts guard-ui -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
tar -czf ../gmv-delivery-split.tar.gz .
```

实际交付前应把 `config/*.yml` 调整为目标现场 IP、域名、证书、数据库和节点 ID。仓库示例配置已保持 MQTT、HTTP integration 和 bootstrap secret 为空；不得把开发环境数据库、日志、broker 凭据、bootstrap 密码或 `guard.integrations.master_key` 打入通用交付包。现场 secret 应通过受控渠道单独注入，并将配置权限限制为服务账号可读。

## 19. 升级和回滚

### 19.1 升级前备份

```bash
sudo systemctl stop gmv-session-gb28181 gmv-stream gmv-guard
tar -czf /opt/gmv-backup-$(date +%Y%m%d%H%M%S).tar.gz /opt/gmv/config /opt/gmv/data
```

### 19.2 分开部署升级

```text
1. 停止服务
2. 替换 bin/guard、bin/gmv-session-gb28181、bin/gmv-stream
3. 替换 guard-ui/dist
4. 保留 config、data、logs、pics、videos
5. 启动服务
6. 验收
```

### 19.3 集成部署升级

```text
1. 停止服务
2. 替换 bin/guard、bin/gmv-session-gb28181、bin/gmv-stream
3. 保留 config、data、logs、pics、videos
4. 启动服务
5. 验收
```

### 19.4 回滚

```text
1. 停止服务
2. 恢复上一版 bin 和必要的前端 dist
3. 不回滚数据库，除非升级说明明确要求且已确认兼容
4. 启动服务
5. 验收
```

## 20. 常见问题

### 20.1 页面打开但 API 401/403

检查：

- 浏览器访问的域名是否在 `guard.http.origins` 中；
- Nginx 是否保留 Host；
- 是否跨域访问 API；
- 登录态 cookie 是否被浏览器接受；
- 写操作是否携带 CSRF header。

### 20.2 Session 注册不上 Guard

检查：

- `session.guard.endpoint` 是否指向 Guard gRPC 地址；
- Guard `guard.grpc.bind_addr` 是否对 Session 可达；
- `domain_id` 是否在 Guard `allowed_nodes` 中；
- gRPC TLS 是否两侧一致；
- 防火墙是否放行端口。

### 20.3 Stream 注册不上 Guard

检查：

- `stream.guard.endpoint` 是否指向 Guard gRPC 地址；
- `server.name` 是否在 Guard `allowed_nodes` 中；
- `server.host` 是否为 Guard/Session 可达地址；
- Stream gRPC 端口是否被占用。

### 20.4 播放地址不可访问

检查：

- `stream.server.proxy_addr` 是否为浏览器可访问前缀；
- Nginx 是否代理 Stream 播放路径；
- Stream `http_port` 是否开放或被 Nginx 正确转发；
- 播放 token 是否过期；
- 浏览器 Network 是否请求公网资源。

### 20.5 构建 Stream 找不到 FFmpeg

执行：

```bash
./stream/build_ffmpeg_min_bootstrap.sh
source ./stream/env_ffmpeg.sh
pkg-config --modversion libavformat
```

然后重新构建：

```bash
~/.cargo/bin/cargo build -p gmv-stream --release
```

### 20.6 构建 Session 找不到 PJSIP

执行：

```bash
./session/gb28181/build_pjsip_bootstrap.sh
ls third_party/pjproject-2.17/dist/include/pjsip.h
ls third_party/pjproject-2.17/dist/lib/libpjsip*.a
```

然后重新构建：

```bash
~/.cargo/bin/cargo build -p gmv-session-gb28181 --release
```

## 21. 第三方应用集成交付

### 21.1 协议边界和接口入口

HTTP 与 MQTT 是并列的外部协议适配器，不存在“以 HTTP 为标准把 MQTT 转成 HTTP”或反向转换。两种入口都完成独立认证、授权、幂等和 DTO 解码后进入 Guard 的 `BusinessControl`；Session、Stream、Avai 继续维护各自业务事实和资源状态机。

| 能力 | 入口 | 身份 | 说明 |
| --- | --- | --- | --- |
| Guard UI | `/api/v2/**` | Cookie Session + CSRF | 供管理员、操作员和查看者使用 |
| 第三方 HTTP | `/openapi/v1/**` | `GMV-HMAC-SHA256-V1` + integration scope | 不创建 Admin UI session，不开放用户、integration 管理和内部 RPC |
| 第三方 MQTT command | `gmv/commands/{integration_id}` | Broker TLS/账号/ACL + Guard 应用级动态授权 | 每条消息检查应用状态、有效期、精确 topic、协议版本和 action |
| MQTT command result | `gmv/command-results/{integration_id}` | Broker ACL | Guard 通过持久化 outbox 发布执行终态 |
| HTTP callback / MQTT event | integration mapping | HMAC 或 Broker ACL | 先进入持久化 outbox，再按策略重试 |
| 在线契约 | `/api-docs/**` | Admin UI session | 只读查看 OpenAPI、AsyncAPI 和 manifest |

第三方离线开发契约由下面的命令生成，内容与在线文档同源：

```bash
/opt/gmv/bin/guard export-integration-contracts /opt/gmv/contracts
```

输出 `openapi.json`、`asyncapi.json`、`manifest.json` 和 `README.md`。导出物不得包含 Access Key、Secret、broker 凭据、master key 或现场 endpoint。

当前契约的 62 个 MQTT action 覆盖 58 个开放 HTTP path、65 个 method/path 业务操作；同一 action 可以对应多个 HTTP 操作。能力映射不是人工维护的表：OpenAPI operation 通过 `x-gmv-mqtt-action`/`x-gmv-mqtt-special` 标出 MQTT 对应项，AsyncAPI 通过 `x-gmv-http-mqtt-capabilities` 和 `x-gmv-action-usage` 给出 action、scope、payload/result schema 与 HTTP 等价接口，manifest 同步导出该矩阵。

明确的协议特例包括：HTTP `GET /events` 以轮询读取事件，MQTT 通过 event Topic 推送；图片、云录像文件和播放媒体等数据面内容不塞入 MQTT payload，MQTT 只执行控制和 access URL/播放 endpoint 签发。除此之外，开放业务能力缺少 MQTT 映射视为契约漂移并应阻断发布。

### 21.2 HTTP HMAC 与幂等

所有请求必须携带 `X-GMV-Access-Key`、`X-GMV-Timestamp`、`X-GMV-Nonce`、`X-GMV-Content-SHA256` 和 `X-GMV-Signature`。canonical request 字段顺序以导出 OpenAPI 的 `x-gmv-hmac.canonical_fields` 为准。

所有 POST 还必须携带 `X-GMV-Request-ID`，并把该值放入 canonical request；GET 对应字段为空字符串。重试时保持相同请求 ID 和相同 method/path/query/body，使用新的 timestamp、nonce 和 signature。同一应用 24 小时内重复已完成请求返回原 HTTP 状态和正文；相同请求 ID 携带不同内容返回 409；处理中请求返回同一 `operation_id`。请求正文不持久化，Guard 只保存摘要和有限期响应重放数据。

部署时必须满足：

- `guard.integrations.master_key` 为 32 字节随机密钥的无填充 Base64；启用 HTTP integration 前必须设置。
- 配置文件权限至少限制为服务账号和管理员可读，不能写入日志、URL、错误 details 或通用交付包。
- 升级、回滚和迁移必须保留原 master key；更换为新值会使历史 credential ciphertext 无法解密。
- integration 停用、过期、scope 收缩或 credential revoke 后，新请求立即失败。
- Nginx 对 `/openapi/v1/**` 只提供 HTTPS、请求体大小和连接限流，不得改写签名使用的 path/query/body。

### 21.3 MQTT Broker、TLS 和运行期授权

MQTT 使用部署级单 Broker 连接。`guard.integrations.mqtt.protocol_version` 选择 `v3`（MQTT 3.1.1）或 `v5`（MQTT 5.0），应用配置必须与当前 runtime 一致；broker 地址、TLS、账号和协议版本变更后重启 Guard。

应用启停、有效期、command topic、`allowed_actions` 和 integration scopes 由数据库维护，每条 command 执行前实时校验，因此这些变更不依赖重启。action 白名单与业务 scope 必须同时满足；例如设备写操作即使在 `allowed_actions` 中存在，缺少对应 `devices:write` 仍会拒绝。Guard 固定接收 `gmv/commands/#` 命名空间，但 wildcard subscription 只负责接收，不授予业务权限；未知 integration/topic/action 必须 fail-closed。

Broker 最小 ACL 示例语义：

```text
Guard subscribe: gmv/commands/+
Guard publish:   gmv/command-results/+
Guard publish:   gmv/events/#
Partner A publish:   gmv/commands/partner-a
Partner A subscribe: gmv/command-results/partner-a
Partner A subscribe: gmv/events/partner-a/#
```

生产环境启用 TLS，服务端证书需要完整校验；QoS 固定为 1，retain 关闭。第三方按 `command_id` 幂等消费 result，按 `event_id` 幂等消费 event。当前注册的 62 个 action、对应 scope、payload/result schema、HTTP 等价接口和逐 action 示例均以当前二进制导出的 `asyncapi.json` 为准；原有九种高频 action 名称与 payload 保持兼容。

### 21.4 MQTT 点播交付与最小调用流程

交付 MQTT 应用时，不能只交付 Broker 地址和 Topic。部署方必须同时交付 `integration_id`、MQTT 版本、TLS/账号、精确 `command_topic`、精确 `result_topic`、event topics、`allowed_actions` 以及由当前二进制导出的 `asyncapi.json` 和 `README.md`。在线入口为 `/api-docs/asyncapi.json`，离线契约通过以下命令生成：

```bash
gmv-guard-server export-integration-contracts ./integration-contracts
```

实时点播的最小流程为：先以 QoS 1 订阅 `result_topic`，再以 QoS 1、`retain=false` 向 `command_topic` 发布命令；收到 `state=succeeded` 后使用 `result.endpoint` 播放并保存 `result.stream_id`；业务结束后以该 `stream_id` 发送 `stream.stop`。PUBACK 不是业务成功。

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
    "stream_profile": "main"
  }
}
```

`trans_mode` 可选 `udp`、`tcp_active`、`tcp_passive`；实时预览 `output_type` 可选 `flv`、`fmp4`、`hls`、`ll_hls`；`stream_profile` 可选 `main`、`sub`。`command_id` 为 1～128 个非空白字符，同一业务重试必须复用；`expires_at_ms - issued_at_ms` 不得超过 300000 ms。

成功 result 至少包含 `stream_id` 和状态，可播放时包含 `endpoint`；失败 result 的 `state=failed`、`result=null`，调用方读取稳定 `error_code`。endpoint 可能包含媒体访问 token，禁止写日志或分发给未授权终端。在线 HTTP 页面会逐 API 展开请求字段、成功/错误响应字段及完整示例；在线 MQTT 页面会逐 action 展开 target、scope、HTTP 等价接口、payload/result 字段和 request/success/failure 示例。离线内容来自同一 OpenAPI/AsyncAPI，不允许部署方另写一份可能漂移的字段表。

### 21.5 网络与持久化

在原端口清单基础上增加：

| 方向 | 目的 | 要求 |
| --- | --- | --- |
| 第三方 -> Nginx/Guard | `/openapi/v1/**` HTTPS | 仅开放必要来源，保留原始 path/query/body |
| 管理员 -> Nginx/Guard | `/api-docs/**` HTTPS | 需要 Admin UI session，不匿名暴露 |
| Guard -> MQTT Broker | TCP/TLS，通常 8883 | 最小账号和 topic ACL |
| Guard -> callback endpoint | HTTPS 443 | DNS/IP/私网策略校验，禁止未授权内网地址和重定向 |

Guard 当前持久化表包括 `_base_db_migrations`、`guard_user`、`guard_outbox`、`guard_command`、`guard_integration`、`guard_integration_credential`、`guard_integration_http`、`guard_integration_mqtt`、`guard_integration_mapping`、`guard_integration_audit`、`guard_integration_delivery`。升级前同时备份数据库和 Guard 配置；不得单独恢复数据库而遗漏对应 master key。

### 21.6 第三方专项验收矩阵

```text
[ ] OpenAPI 含 58 个开放 path，且不含用户、角色、integration 管理和内部 RPC
[ ] POST 缺失 X-GMV-Request-ID 时返回 400
[ ] HMAC 正确请求成功，错误签名、过期 timestamp 和 nonce replay 被拒绝
[ ] 同一请求 ID/相同内容跨 Guard 重启返回原响应
[ ] 同一请求 ID/不同内容返回 409
[ ] MQTT V3 或 V5 与现场 runtime 版本一致
[ ] 58 个开放 HTTP path、65 个 method/path operation 均映射到 MQTT action 或登记的事件 Topic 特例
[ ] 62 个 MQTT action 均有 scope、payload/result schema、request/success/failure 示例和实际 executor 映射
[ ] 原有九种 MQTT action 名称与 payload 兼容，旧客户端回归通过
[ ] 未知 topic、错误 integration_id、停用/过期应用和未授权 action 被拒绝
[ ] 应用停用、allowed_actions 或 scopes 收缩无需重启立即生效
[ ] command result 包含 schema_version、integration_id、command_id、operation_id、state、error_code、occurred_at_ms
[ ] HTTP callback 签名、超时、重试、失败终态和重启恢复通过
[ ] MQTT event mapping、outbox retry、event_id 幂等和重启恢复通过
[ ] HTTP/MQTT 播放票据续期只允许票据 owner，达到绝对期限或次数上限后拒绝
[ ] 在线文档仅管理员可见，导出契约可在无 UI session 环境解析
[ ] 在线 HTTP/MQTT 每个 API/Action 均展示请求字段、响应字段和完整请求/成功/失败示例
[ ] 交付包不含数据库、日志、secret、私钥、broker 凭据和前端 sourcemap
```

真实生产 broker、物理 GB28181 设备、外部 callback 和现场网络策略必须在部署环境完成最后验收；本地未执行的项目不得写成通过。

## 22. 发布前检查清单

构建检查：

```text
[ ] guard/ui 已执行 pnpm install --frozen-lockfile
[ ] guard/ui 已执行 pnpm build
[ ] guard embed-ui 构建通过或分开部署 guard 构建通过
[ ] session-gb28181 构建通过
[ ] stream 构建通过
[ ] 目标平台二进制已在目标设备或同架构环境验证
[ ] ldd/file 检查完成
```

交付检查：

```text
[ ] bin/ 包含 guard、gmv-session-gb28181、gmv-stream
[ ] config/ 包含 guard.yml、session-gb28181.yml、stream.yml
[ ] 分开部署时包含 guard-ui/dist
[ ] 集成部署时 guard 使用 --features embed-ui 构建
[ ] systemd 文件中的 WorkingDirectory 和 ExecStart 路径正确
[ ] Nginx 域名、证书、反代路径正确
[ ] SHA256SUMS 已生成
[ ] VERSION 已记录版本、commit、目标平台、构建时间
[ ] contracts/ 包含 OpenAPI、AsyncAPI、manifest 和第三方 README
[ ] config/*.yml 已按目标环境生成，未直接复制开发凭据
```

安全检查：

```text
[ ] guard-server 未直接暴露到不可信网络，或仅开放必要端口
[ ] /metrics 已限制访问
[ ] bootstrap 初始密码已修改或移除
[ ] 配置文件和证书私钥权限正确
[ ] ui_dist_dir 未指向敏感目录
[ ] 前端 dist 不含 sourcemap
[ ] guard.integrations.master_key 已通过受控渠道注入并纳入升级备份
[ ] MQTT 已启用 TLS、独立账号和最小 topic ACL
[ ] 制品扫描确认不含数据库、日志、Access Key、Secret、broker 凭据或私钥
```

验收检查：

```text
[ ] /health/live 正常
[ ] /health/ready 正常
[ ] UI 可登录
[ ] Session 节点在线
[ ] Stream 节点在线
[ ] GB28181 设备接入正常
[ ] 预览/回放链路正常
[ ] 重启服务后自动恢复
[ ] 第三方 HTTP HMAC、请求 ID 幂等和跨重启重放通过
[ ] MQTT command/result、应用即时停用和权限收缩通过
[ ] HTTP callback/MQTT event outbox 重试与恢复通过
[ ] 在线文档和离线契约导出物逐项核对通过
```
