# GmvPlayer 实现计划

## 目标

在 `guard` 工程下新增独立的 `player` 子工程，用于封装 GMV Web 播放器能力，并与 `guard/ui`、GB28181 业务页面、流媒体控制 API 解耦。

播放器采用两层架构：

```text
GmvPlayerCore：不带 UI，只负责播放、能力探测、协议适配、状态事件、重连和资源释放。
GmvPlayerView：GMV 自研 UI，负责安防场景交互、多画面、控制面板和业务操作入口。
```

核心原则：

- `GmvPlayerCore` 不依赖 Vue、Element Plus、业务 API、路由、状态管理。
- `GmvPlayerView` 可以适配 Vue，但通过明确接口调用 Core，不直接绑定某个播放库。
- HTTP-FLV、HTTP-FMP4、HLS-fMP4 分别由独立 engine 适配，外部只感知统一的 `GmvPlayer`。
- 当前已支持实时预览和 HTTP-FLV / HTTP-FMP4 / HLS-fMP4；HLS 同时提供普通与低延迟两个 playlist endpoint。

## 关键假设

- 当前 `guard/ui` 是 Vue 3 + Vite + Element Plus。
- 当前 GB28181 预览接口可返回 `StreamSummary.endpoint`。
- 当前 GB28181 `output_type` 支持 `flv`、`fmp4`、普通 `hls` 和直播专用 `ll_hls`。
- Stream 已实现普通 HLS 与 LL-HLS；两者共享 CMAF 媒体，播放器依据 endpoint 选择普通或低延迟配置。
- 第一阶段只新增 player 子工程，不直接修改现有 GB28181 页面业务逻辑。

## 建议目录

```text
guard/player
├── README.md
├── package.json
├── tsconfig.json
├── vite.config.ts
├── src
│   ├── core
│   │   ├── GmvPlayerCore.ts
│   │   ├── types.ts
│   │   ├── capability
│   │   │   ├── BrowserProbe.ts
│   │   │   └── CodecProbe.ts
│   │   ├── engines
│   │   │   ├── BaseEngine.ts
│   │   │   ├── FlvEngine.ts
│   │   │   ├── Fmp4Engine.ts
│   │   │   └── HlsEngine.ts
│   │   └── utils
│   │       ├── EventBus.ts
│   │       ├── ErrorCode.ts
│   │       └── BufferStats.ts
│   ├── view
│   │   ├── GmvPlayerView.vue
│   │   ├── MultiGrid.vue
│   │   ├── ControlBar.vue
│   │   ├── PtzPanel.vue
│   │   ├── PresetPanel.vue
│   │   ├── PlaybackTimeline.vue
│   │   ├── StreamSwitcher.vue
│   │   ├── DeviceStatusBar.vue
│   │   ├── OsdLayer.vue
│   │   ├── AiOverlay.vue
│   │   └── ReconnectBanner.vue
│   └── index.ts
├── examples
│   └── basic-preview
└── tests
    ├── core
    └── view
```

首期可以只创建 `src/core` 和最小 `src/view/GmvPlayerView.vue`，其余组件按功能阶段逐步补齐。

## Core 设计

### Source 模型

```ts
export type GmvProtocol = 'flv' | 'fmp4' | 'hls';
export type GmvCodec = 'h264' | 'h265';

export interface GmvSource {
  protocol: GmvProtocol;
  url: string;
  codec?: GmvCodec;
  mimeCodec?: string;
  hasAudio?: boolean;
  priority?: number;
  label?: string;
}

export interface GmvPlayerCoreOptions {
  video: HTMLVideoElement;
  sources: GmvSource[];
  autoplay?: boolean;
  muted?: boolean;
  lowLatency?: boolean;
  fallback?: boolean;
}
```

### Engine 接口

```ts
export interface GmvEngine {
  readonly protocol: GmvProtocol;
  attach(video: HTMLVideoElement, source: GmvSource): Promise<void> | void;
  play(): Promise<void> | void;
  pause(): void;
  destroy(): void;
}
```

### 协议适配

- `FlvEngine`：使用 `mpegts.js`，用于 HTTP-FLV / WS-FLV。
- `Fmp4Engine`：使用原生 `MediaSource + SourceBuffer + fetch ReadableStream`，用于 HTTP-FMP4 长连接流。
- `HlsEngine`：Apple Safari 或无 MSE 的原生 HLS 环境使用 `<video>` 原生路径；具备 MSE 的 Chromium 使用 `hls.js`，不只依赖 `canPlayType()` 的声明。只有 `.ll.m3u8` endpoint 启用 hls.js low-latency 模式，普通 `.m3u8` 保持兼容配置。
- `CapabilityProbe`：统一探测 `MediaSource.isTypeSupported`、`MediaCapabilities.decodingInfo`、`video.canPlayType`、`mpegts.getFeatureList`、`Hls.isSupported`。

### Core 事件

```ts
export interface GmvPlayerEvents {
  loading: undefined;
  playing: undefined;
  paused: undefined;
  stalled: undefined;
  reconnecting: { retry: number; reason: string };
  reconnected: undefined;
  error: { code: string; message: string; source?: GmvSource };
  sourceChanged: { source: GmvSource };
  stats: {
    protocol: GmvProtocol;
    codec?: GmvCodec;
    bitrate?: number;
    fps?: number;
    bufferSeconds?: number;
    viewers?: number;
  };
  destroyed: undefined;
}
```

### Core 职责边界

Core 负责：

- source 选择和 fallback。
- 播放、暂停、销毁。
- engine 生命周期管理。
- 浏览器能力探测。
- 断线重连策略。
- 缓冲区清理。
- 播放状态和错误事件。
- 自动播放被浏览器拒绝时保留已 attach 的 engine，让 FLV/mpegts 仍有机会完成网络加载。

Core 不负责：

- PTZ 请求。
- 抓拍、录像等业务 API。
- UI 布局。
- 设备列表、通道列表、用户权限。
- AI 框数据来源。

## View 设计

`GmvPlayerView` 是 GMV 自研安防播放器 UI，组合 Core 和业务 action。

### View 输入

```ts
export interface GmvPlayerViewProps {
  sources: GmvSource[];
  deviceId?: string;
  channelId?: string;
  title?: string;
  status?: GmvDeviceStatus;
  viewers?: number;
  poster?: string;
  osd?: GmvOsdItem[];
  aiBoxes?: GmvAiBox[];
  capabilities?: GmvViewCapabilities;
}
```

### View 输出事件

```ts
export interface GmvPlayerViewActions {
  snapshot: { deviceId?: string; channelId?: string };
  recordStart: { deviceId?: string; channelId?: string };
  recordStop: { deviceId?: string; channelId?: string };
  ptz: GmvPtzCommand;
  presetCall: { presetId: string };
  presetSet: { presetId: string };
  playbackSeek: { timeMs: number };
  streamSwitch: { source: GmvSource };
}
```

业务页面只监听这些事件，再调用 `guard/ui/src/api/client.ts` 中的业务 API。View 不直接 import 业务 API。

## UI 功能拆分

### 多宫格

目标：

- 支持 1 / 4 / 9 / 16 宫格。
- 每个格子持有一个 `GmvPlayerCore` 实例。
- 支持选中主画面、双击放大、空画面占位。
- 支持单个画面销毁，不影响其他画面。

验收：

- 切换宫格不泄漏旧 video / MediaSource / fetch 请求。
- 多路播放时每路状态独立。

### PTZ 控制

目标：

- 支持上、下、左、右、左上、右上、左下、右下、停止。
- 支持变倍、聚焦、光圈。
- 支持速度参数。
- 鼠标松开或触控结束自动发送停止。

验收：

- 连续按压不会堆积不可控请求。
- 停止命令优先发送。

### 预置点

目标：

- 支持预置点调用。
- 支持预置点设置。
- 支持预置点删除，删除属于危险操作，业务层需要确认。

验收：

- View 只发出 `presetCall`、`presetSet`、`presetDelete` 事件。
- 权限和确认逻辑留在业务页面。

### 抓拍

目标：

- 支持业务后端抓拍。
- 可选支持前端当前帧截图。
- 抓拍结果由业务层刷新图片列表。

验收：

- View 不直接依赖 GB28181 图片接口。
- 抓拍按钮有 loading / disabled 状态。

### 录像

目标：

- 支持开始录像、停止录像。
- 显示录像状态和持续时间。
- 录像存储位置、文件名、权限由业务层处理。

验收：

- View 只负责交互和状态展示。
- Core 不参与录像业务。

### 回放时间轴

目标：

- 支持时间范围选择。
- 支持播放、暂停、倍速、拖动定位。
- 支持录像片段高亮。
- 支持直播和回放模式切换。

验收：

- 时间轴组件只产生 `playbackSeek`、`rangeChange`、`speedChange` 事件。
- 具体回放流创建由业务层调用接口完成。

### 码流切换

目标：

- 支持主码流 / 子码流 / 第三码流。
- 支持 H.264 / H.265 fallback。
- 支持 FLV / FMP4 / HLS 协议切换。

验收：

- 切换 source 时旧 engine 完整销毁。
- 切换失败可回退到上一路可播放 source。

### 设备状态

目标：

- 展示在线、离线、播放中、断线、重连中、无权限、设备忙。
- 支持信号质量、码率、帧率、延迟等扩展指标。

验收：

- 播放状态来自 Core。
- 设备业务状态来自业务层 props。

### OSD

目标：

- 支持通道名、时间、位置、码流、告警文字叠加。
- 支持固定位置和业务传入文本。
- 不遮挡关键控制按钮。

验收：

- OSD 只做展示，不改变视频内容。
- 全屏和宫格模式位置一致。

### AI 框

目标：

- 支持矩形框、多边形框、标签、置信度。
- 支持按视频渲染尺寸进行坐标缩放。
- 支持开关显示。

验收：

- AI overlay 与 video 尺寸同步。
- 画面 resize 后框位置不漂移。

### 断线重连

目标：

- 支持指数退避重连。
- 支持最大重试次数。
- 支持手动重连。
- 支持重连期间保持 UI 状态。

验收：

- fetch / MediaSource / mpegts / hls 资源在重连前释放。
- 重连失败后抛出明确错误事件。

### 当前观看人数

目标：

- 支持显示当前观看人数。
- 数据来源由业务层传入或由 stats 事件更新。
- 多宫格下每路独立展示。

验收：

- 无观看人数数据时隐藏或显示 `-`。
- 不由播放器主动请求业务接口。

## 分阶段计划

### 阶段一：工程和 Core 最小闭环

范围：

- 新增 `guard/player` 子工程。
- 建立 TypeScript 构建配置。
- 实现 `GmvPlayerCore`、`BaseEngine`、`EventBus`、基础类型。
- 实现 `FlvEngine`。
- 提供 basic preview 示例。

验收：

- 可使用 HTTP-FLV 播放一路实时预览。
- `destroy()` 后无继续拉流。
- 类型检查和构建通过。

### 阶段二：HTTP-FMP4

范围：

- 实现 `Fmp4Engine`。
- 处理 init segment、chunk queue、串行 append、AbortController。
- 增加 buffer cleanup。

验收：

- 可播放 GMV `.fmp4` 长连接流。
- SourceBuffer append 不并发。
- 断流后能释放 fetch 和 MediaSource。

### 阶段三：能力探测和 fallback

范围：

- 实现浏览器能力探测。
- 实现 source 排序。
- 实现播放失败 fallback。
- 增加 H.265 不支持时回退 H.264 的策略。

验收：

- 不支持当前 source 时自动选择下一路。
- 错误事件包含协议、编码、URL 和失败原因。

### 阶段四：GmvPlayerView 基础 UI

范围：

- 实现单画面播放器。
- 实现控制栏、状态栏、OSD、断线重连提示。
- 实现抓拍、录像、码流切换事件。

验收：

- View 不直接调用业务 API。
- `guard/ui` 可以通过事件接入现有接口。

### 阶段五：安防 UI 完整能力

范围：

- 多宫格。
- PTZ。
- 预置点。
- 回放时间轴。
- AI 框。
- 当前观看人数。

验收：

- 多宫格每路生命周期独立。
- PTZ / 预置点 / 抓拍 / 录像均通过事件交给业务层。
- UI 在桌面端和常见大屏分辨率下不重叠。

### 阶段六：HLS-fMP4

范围：

- 实现 `HlsEngine`。
- Safari / iOS 使用 native HLS。
- 普通 `{stream_id}.m3u8` 与低延迟 `{stream_id}.ll.m3u8` 共享媒体数据。

验收：

- `.m3u8` 可播放。
- 非 Safari 使用 hls.js。
- Safari / iOS 使用 video 原生能力。
- 普通 HLS 不启用 hls.js low-latency 模式；LL-HLS 启用 blocking reload 与低延迟追赶。

## 依赖建议

首期生产依赖：

```text
hls.js：HLS-fMP4
vue：GmvPlayerView 和人工测试页
自研 EventBus：播放器事件系统
```

HTTP-FLV 适配器直接依赖 mpegts.js。构建 guard/ui 或 guard/player 时，mpegts.js 会随 Vite/Rollup 进入本地 dist 产物，不需要宿主工程或部署环境额外提供运行时库。

调试依赖：

```text
mp4box：调试 init segment、hvcC、avcC、track、timescale
```

暂不作为默认核心：

```text
ffmpeg.wasm：包体大，CPU 开销高，不适合多路实时预览。
h265web.js：可作为实验性 H.265 软解兜底，不进入首期默认链路。
video.js / xgplayer：可参考 UI，不作为 Core 强依赖。
```

## 与 guard/ui 的集成方式

`guard/ui` 业务页面负责：

- 调用预览、回放、PTZ、抓拍、录像和语音广播 API；语音广播不通过播放器组件发起。
- 把接口返回的 `endpoint` 转成 `GmvSource[]`。
- 把设备状态、AI 结果、观看人数传入 `GmvPlayerView`。
- 处理权限、确认弹窗、错误提示。

`guard/player` 负责：

- 播放器 core。
- 播放器 UI。
- 播放事件和 UI 操作事件。
- 示例和测试。

后续可通过 workspace、相对路径依赖或构建产物接入 `guard/ui`，具体方式在工程初始化时确认。

## 后端协作点

需要后端或业务 API 提供：

- 标准化播放源信息：协议、URL、videoCodec、audioCodec、mimeCodec、清晰度、码流名。
- H.264/H.265 codec string，避免前端硬编码；无法识别时由播放器按 AUTO 路径尝试。
- HLS-fMP4 输出能力。
- 观看人数查询或推送。
- AI 框数据坐标系和时间戳。
- 回放录像片段列表。
- PTZ、预置点、抓拍、录像 API。

## 验证方式

每个阶段至少执行：

```bash
pnpm -C guard/player typecheck
pnpm -C guard/player build
pnpm -C guard/player test
```

人工播放测试页面：

```bash
pnpm -C guard/player dev
```

打开 Vite 输出地址后，在页面中输入 FLV / FMP4 / HLS 播放 URL、协议、编码和 FMP4 mimeCodec，加入宫格画面进行人工播放测试。

接入 `guard/ui` 后追加：

```bash
pnpm -C guard/ui typecheck
pnpm -C guard/ui build
```

人工验证：

- FLV 实时预览。
- FLV + G711A/G711U 音频流可通过 hasAudio=false 按视频优先播放。
- FMP4 实时预览。
- source 切换。
- 播放失败 fallback。
- destroy 后无残留网络请求。
- 多宫格切换。
- 全屏、OSD、AI 框、控制栏无重叠。

## 操作控件配置

`GmvPlayerView` 通过 `controls` 决定展示哪些操作，通过 `capabilities` 决定已展示操作当前是否可用。两者职责不同：

- `controls.items`：直接展示在画面底部的主操作，数组顺序即从左到右顺序。
- `controls.overflowItems`：点击“…”后在当前画面内竖向展示的扩展操作，数组顺序即从上到下顺序；数组为空时不展示“…”按钮。
- `capabilities`：运行时能力。已经配置但当前不可用的操作显示为 disabled；未配置的操作不创建 DOM。
- `controls.visibility`：`auto` 为播放中 3 秒无操作后隐藏，`always` 为始终显示，`hidden` 为不渲染。

```vue
<GmvPlayerView
  :sources="sources"
  :capabilities="{
    snapshot: true,
    audio: true,
    ptz: true,
    record: false,
  }"
  :controls="{
    items: ['play', 'snapshot', 'fullscreen', 'ptz'],
    overflowItems: ['audio'],
    visibility: 'auto',
    autoHideDelayMs: 3000,
  }"
  @snapshot="handleSnapshot"
/>
```

通用 `GmvPlayerControls` 只发出类型化 `GmvPlayerControlAction`，不直接持有 `GmvPlayerCore`，也不调用业务 API。`GmvPlayerView` 负责把播放、声音、倍速和全屏 action 映射到本地播放器，把截图、录像、PTZ、预置点、回放定位和码流切换继续通过原有业务事件交给调用方。

`controlsVisible` 仅保留一版兼容：`true` 映射为 `always`，`false` 映射为 `hidden`。新代码应使用 `controls.visibility`。

### 自动显隐

- 播放窗口发生 pointer move、点击或触碰时立即显示并重新计时。
- 正常播放且无操作 3 秒后淡出；隐藏后不拦截画面 pointer 事件。
- 暂停、idle、错误和重连时保持显示。
- hover、键盘焦点、下拉选择、滑块拖动、PTZ 持续操作和“…”菜单展开期间不隐藏。
- PTZ 面板默认关闭；点击“云台”按钮展开，再次点击关闭。
- 自动隐藏时同时关闭 PTZ 面板；重新移动鼠标只恢复操作栏，不自动重新展开 PTZ。
- 顶部标题、已展开的 PTZ 面板和底部操作栏同步显隐；错误、重连和录像状态独立展示。

### 多画面

`GmvGridCell.controls` 和 `GmvGridCell.capabilities` 均属于单个 cell。每一路应根据对应设备、通道、媒体流和业务权限独立生成配置，禁止用共享可变对象覆盖全部画面。

支持拖拽排序的调用方必须为 `GmvGridCell.cellId` 提供工作台内稳定且唯一的值。`MultiGrid` 使用该身份移动既有播放器节点；排序不得把另一通道的 `sources` 写入原槽位播放器，否则会重建媒体引擎并断开已有 HTTP 输出连接。

需要跨页保持媒体连接时，调用方应传入完整 `cells`，并用 `visibleStart` 指定当前页首个 cell。`MultiGrid` 只隐藏非当前页节点，不卸载播放器；事件和公开确认方法继续使用当前页局部 index。

单路全屏、截图、声音、云台及“…”扩展菜单均渲染在该路 player cell 内。多画面把具备能力的“云台”放入对应 cell 的扩展菜单；不支持 PTZ 的 cell 不展示该按钮。宫格档位、分页和整个多画面区域满屏属于容器操作，不属于单路媒体能力。

多画面回放时，`GmvGridCell` 可分别传入 `mediaMode: 'playback'`、`playbackDurationMs`、`playbackStartTimeMs` 和 `playbackEndTimeMs`。`MultiGrid` 会为 `playbackSeek`、`playbackRateChange`、`playbackStateChange`、`playbackProgress` 附加 cell index，并通过 `confirmPlaybackRate(index, rate)`、`confirmPlaybackState(index, paused)`、`confirmPlaybackProgress(index, timeMs)` 只确认对应子播放器。业务层仍负责 playback_id、stream_id、generation、远端控制和资源释放，组件不把多个 cell 合并为共享回放会话。

## 风险和约束

- H.265 能否播放取决于浏览器、系统和硬件解码能力，前端不能保证所有环境可播。
- `MediaSource.isTypeSupported()` 返回 true 也不代表一定可播放，必须保留 fallback。
- FLV 首次 `video.play()` 被拒绝不等于播放源打开失败；不能因此立即销毁 `mpegts.js` engine，否则可能在发起媒体请求前中断。
- HTTP-FLV 的 G711A/G711U 音频不由 mpegts.js 解码，遇到这类流应设置 hasAudio=false，只播放视频轨。
- HTTP-FMP4 要求服务端 init segment 在前，后续 moof / mdat 时间戳连续。
- 多宫格会放大 CPU、内存、网络和解码压力，默认应限制同时播放路数。
- 语音广播涉及麦克风权限和独立下行媒体链路，由 `guard/ui` 业务层实现，不属于播放器能力。
- LL-HLS 的最终兼容性仍需以最低支持 iOS、当前 iOS 和当前 macOS Safari 真机验证为准；桌面 Chromium/hls.js 测试不能替代 Apple 真机验收。

## 最小首期验收标准

- `guard/player` 子工程可独立构建。
- `GmvPlayerCore` 可播放 HTTP-FLV。
- `GmvPlayerCore` 可销毁并释放资源。
- `GmvPlayerView` 有单画面基础 UI。
- 提供人工播放测试页面，可手动输入播放 URL 并进行 FLV / FMP4 / HLS 播放验证。
- UI 操作通过事件抛出，不直接调用业务 API。
- 文档、类型和示例能指导 `guard/ui` 接入。

## GB28181 主/辅码流

`GmvPlayerView` 支持 `streamProfile`、`streamProfileVerification`、`streamProfileOptions` 和 `streamProfileSwitching`。启用 `streamProfile` control 后，播放器发出 `streamProfileChange`；业务层负责创建目标流并更新 source。信息面板展示当前 effective profile，`networkDegraded` 仅表示建议业务层提示用户，不会自动降码率。

Core 的 `playing` 事件经过视频帧栅栏确认；业务层可据此提交双槽切换并释放旧 subscription。
