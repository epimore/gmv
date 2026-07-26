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
target/release/guard
guard/server/config.yml
guard/ui/dist/
guard/server/migrations/manual/mysql/cleanup_legacy_preview_schema.sql
guard/server/migrations/manual/sqlite/cleanup_legacy_preview_schema.sql
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
  share/
    sql/
      mysql/
        cleanup_legacy_preview_schema.sql
      sqlite/
        cleanup_legacy_preview_schema.sql
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
target/release/guard
guard/server/config.yml
guard/server/migrations/manual/mysql/cleanup_legacy_preview_schema.sql
guard/server/migrations/manual/sqlite/cleanup_legacy_preview_schema.sql
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
| Guard | `db-all` | `target/release/guard` | 21,014,272 | 20.04 | 基准 |
| Guard | `db-mysql` | `target/release/guard` | 17,064,184 | 16.27 | -3.77 MiB |
| Guard | `db-sqlite` | `target/release/guard` | 17,871,048 | 17.04 | -3.00 MiB |
| Guard | `db-all,embed-ui` | `target/release/guard` | 24,134,712 | 23.02 | +2.98 MiB |

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
ldd target/release/guard
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
file target/x86_64-unknown-linux-musl/release/guard
ldd target/x86_64-unknown-linux-musl/release/guard || true
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

需要显式清理旧预览数据库中的废弃 Guard 对象时，必须先停止 Guard 写入并完成备份，再按实际后端执行人工脚本：

```bash
# SQLite：先复制 guard.db 形成可恢复备份
sqlite3 /opt/gmv/data/guard.db ".read /opt/gmv/share/sql/sqlite/cleanup_legacy_preview_schema.sql"

# MySQL：DDL 会隐式提交，不能依赖事务回滚
mysql -h 127.0.0.1 -u gmv -p gmv < /opt/gmv/share/sql/mysql/cleanup_legacy_preview_schema.sql
```

脚本只删除 `guard_node`、`guard_lease`、`guard_route`、`guard_event`、`guard_service_credential`、`guard_ui_session`、`guard_integration`、`guard_system_setting`，并精确删除对应旧 Guard 迁移账本记录。当前旧表没有独立命名的 secondary index；主键索引随 `DROP TABLE` 一并删除。脚本保留 `_base_db_migrations`、`guard_user`、`guard_outbox`、`guard_command` 及其他模块的账本记录。人工脚本不是启动 migration，也不是旧库兼容承诺。

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
mkdir -p /tmp/gmv-delivery/{bin,config,systemd,nginx,certs,data,logs,pics,videos}

cp target/release/guard /tmp/gmv-delivery/bin/
cp target/release/gmv-session-gb28181 /tmp/gmv-delivery/bin/
cp target/release/gmv-stream /tmp/gmv-delivery/bin/

cp guard/server/config.yml /tmp/gmv-delivery/config/guard.yml
cp session/gb28181/config.yml /tmp/gmv-delivery/config/session-gb28181.yml
cp stream/config.yml /tmp/gmv-delivery/config/stream.yml
cp PACKAGING_DEPLOYMENT.md /tmp/gmv-delivery/

git rev-parse HEAD > /tmp/gmv-delivery/VERSION
date -u +"build_time_utc=%Y-%m-%dT%H:%M:%SZ" >> /tmp/gmv-delivery/VERSION

cd /tmp/gmv-delivery
find bin config -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
tar -czf ../gmv-delivery-integrated.tar.gz .
```

### 18.2 分开部署交付包

```bash
cd /home/ubuntu20/code/rs/mv/github/epimore/gmv

rm -rf /tmp/gmv-delivery
mkdir -p /tmp/gmv-delivery/{bin,config,systemd,nginx,certs,data,logs,pics,videos,guard-ui}

cp target/release/guard /tmp/gmv-delivery/bin/
cp target/release/gmv-session-gb28181 /tmp/gmv-delivery/bin/
cp target/release/gmv-stream /tmp/gmv-delivery/bin/
cp -a guard/ui/dist /tmp/gmv-delivery/guard-ui/

cp guard/server/config.yml /tmp/gmv-delivery/config/guard.yml
cp session/gb28181/config.yml /tmp/gmv-delivery/config/session-gb28181.yml
cp stream/config.yml /tmp/gmv-delivery/config/stream.yml
cp PACKAGING_DEPLOYMENT.md /tmp/gmv-delivery/

git rev-parse HEAD > /tmp/gmv-delivery/VERSION
date -u +"build_time_utc=%Y-%m-%dT%H:%M:%SZ" >> /tmp/gmv-delivery/VERSION

cd /tmp/gmv-delivery
find bin config guard-ui -type f -print0 | sort -z | xargs -0 sha256sum > SHA256SUMS
tar -czf ../gmv-delivery-split.tar.gz .
```

实际交付前应把 `config/*.yml` 调整为目标现场 IP、域名、证书、数据库和节点 ID。

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

## 21. 发布前检查清单

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
```

安全检查：

```text
[ ] guard-server 未直接暴露到不可信网络，或仅开放必要端口
[ ] /metrics 已限制访问
[ ] bootstrap 初始密码已修改或移除
[ ] 配置文件和证书私钥权限正确
[ ] ui_dist_dir 未指向敏感目录
[ ] 前端 dist 不含 sourcemap
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
```
