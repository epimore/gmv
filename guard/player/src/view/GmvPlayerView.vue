<template>
  <section
    ref="playerRef"
    class="gmv-player"
    :class="[
      'is-' + viewState,
      {
        'player-chrome-hidden': !controlsAreVisible,
        'has-playback-timeline': effectiveControls.items.includes('timeline'),
      },
    ]"
    @pointermove="notifyControlsActivity"
    @pointerdown="notifyControlsActivity"
    @pointerleave="handlePlayerPointerLeave"
    @keydown="notifyControlsActivity"
  >
    <video ref="videoARef" class="gmv-video" :class="{ 'is-active': activeVideoSlot === 0 }" playsinline muted :poster="poster || undefined"></video>
    <video ref="videoBRef" class="gmv-video" :class="{ 'is-active': activeVideoSlot === 1 }" playsinline muted :poster="poster || undefined"></video>

    <div class="gmv-layer osd-layer">
      <span
        v-for="item in osd"
        :key="item.id"
        class="osd-item"
        :style="{ left: item.x + '%', top: item.y + '%' }"
      >
        {{ item.text }}
      </span>
    </div>

    <div v-if="capabilities.aiOverlay !== false" class="gmv-layer ai-layer">
      <span
        v-for="box in aiBoxes"
        :key="box.id"
        class="ai-box"
        :style="boxStyle(box)"
      >
        {{ box.label }}{{ box.confidence ? ' ' + Math.round(box.confidence * 100) + '%' : '' }}
      </span>
    </div>

    <div v-if="isLoading && !activePlaybackReady" class="player-waiting-cover" aria-label="视频加载中" role="status">
      <span class="waiting-ring"></span>
      <span class="waiting-scan"></span>
      <span v-if="startupProgress && startupProgress.elapsedMs >= 3000" class="startup-progress-text">{{ startupProgressText }}</span>
    </div>

    <div v-if="activePlaybackReady && (isLoading || outputSwitching)" class="reconnect-banner startup-switch-banner">
      <span>{{ startupText || startupProgressText || '正在切换播放方式，当前画面继续播放' }}</span>
      <button v-if="startupCanCancel || (startupProgress && startupProgress.elapsedMs >= startupProgress.checkpointMs)" type="button" @click="cancelPendingPlaybackSwitch">保持当前播放</button>
    </div>

    <div v-else-if="viewState === 'reconnecting' || lastError" class="reconnect-banner">
      <span>{{ lastError || '正在重连...' }}</span>
      <button type="button" @click="reconnect">重连</button>
    </div>

    <div v-if="recording" class="recording-indicator" aria-live="polite">● 录像中</div>

    <aside v-if="infoOpen" class="media-info-panel" aria-label="媒体基本信息">
      <div class="media-info-row"><span>名称</span><b>{{ title || '-' }}</b></div>
      <div class="media-info-row"><span>设备/通道</span><b>{{ deviceId || '-' }} / {{ channelId || '-' }}</b></div>
      <div class="media-info-row"><span>播放模式</span><b>{{ mediaModeLabel }}</b></div>
      <div class="media-info-row"><span>媒体输出</span><b>{{ outputFormatLabel }}</b></div>
      <div class="media-info-row"><span>视频</span><b>{{ videoInfoText }}</b></div>
      <div class="media-info-row"><span>音频</span><b>{{ audioInfoText }}</b></div>
      <div class="media-info-row"><span>状态</span><b>{{ statusViewersText }}</b></div>
      <template v-if="mediaMode === 'playback'">
        <div class="media-info-row"><span>回放范围</span><b>{{ playbackRangeText }}</b></div>
        <div class="media-info-row"><span>当前位置</span><b>{{ playbackPositionText }}</b></div>
        <div class="media-info-row"><span>播放倍速</span><b>{{ playbackRate }}x</b></div>
      </template>
      <details class="media-info-diagnostics">
        <summary>扩展</summary>
        <div class="media-info-diagnostic-list">
          <div class="media-info-row"><span>Stream ID</span><b>{{ streamId || '-' }}</b></div>
          <div class="media-info-row"><span>媒体节点</span><b>{{ mediaNodeId || '-' }}</b></div>
          <div class="media-info-row"><span>Session 节点</span><b>{{ sessionNodeId || '-' }}</b></div>
          <div class="media-info-row"><span>已解码帧</span><b>{{ mediaStats.totalFrames || '-' }}</b></div>
          <div class="media-info-row"><span>丢帧</span><b>{{ droppedFramesText }}</b></div>
          <div class="media-info-row"><span>缓冲时长</span><b>{{ mediaStats.bufferSeconds.toFixed(1) }} 秒</b></div>
          <div class="media-info-row"><span>渲染帧率</span><b>{{ mediaStats.fps ? mediaStats.fps.toFixed(1) + ' FPS' : '-' }}</b></div>
          <div class="media-info-row"><span>输出状态</span><b>{{ outputStatusLabel }}</b></div>
        </div>
      </details>
    </aside>

    <aside
      v-if="capabilities.ptz !== false && ptzOpen"
      class="ptz-panel"
      @pointerdown="setControlsInteraction(true)"
      @pointerup="setControlsInteraction(false)"
      @pointercancel="setControlsInteraction(false)"
      @pointerleave="setControlsInteraction(false)"
      @click.stop
    >
      <div class="ptz-grid">
        <button type="button" title="左上" @pointerdown="ptz('leftUp')" @pointerup="ptzStop" @pointerleave="ptzStop">↖</button>
        <button type="button" title="上" @pointerdown="ptz('up')" @pointerup="ptzStop" @pointerleave="ptzStop">↑</button>
        <button type="button" title="右上" @pointerdown="ptz('rightUp')" @pointerup="ptzStop" @pointerleave="ptzStop">↗</button>
        <button type="button" title="左" @pointerdown="ptz('left')" @pointerup="ptzStop" @pointerleave="ptzStop">←</button>
        <button type="button" title="停止" @click="ptzStop">■</button>
        <button type="button" title="右" @pointerdown="ptz('right')" @pointerup="ptzStop" @pointerleave="ptzStop">→</button>
        <button type="button" title="左下" @pointerdown="ptz('leftDown')" @pointerup="ptzStop" @pointerleave="ptzStop">↙</button>
        <button type="button" title="下" @pointerdown="ptz('down')" @pointerup="ptzStop" @pointerleave="ptzStop">↓</button>
        <button type="button" title="右下" @pointerdown="ptz('rightDown')" @pointerup="ptzStop" @pointerleave="ptzStop">↘</button>
      </div>
      <label>
        速度
        <input v-model.number="ptzSpeed" min="1" max="255" type="range" />
      </label>
      <div class="lens-row">
        <button type="button" @click="ptz('zoomIn')">变倍+</button>
        <button type="button" @click="ptz('zoomOut')">变倍-</button>
      </div>
      <div class="lens-row">
        <button type="button" @click="ptz('focusNear')">聚焦近</button>
        <button type="button" @click="ptz('focusFar')">聚焦远</button>
      </div>
    </aside>

    <PlayerControls
      ref="controlsRef"
      :config="effectiveControls"
      :state="controlsState"
      :capabilities="capabilities"
      :sources="sources"
      :output-options="outputOptions"
      :output-switching="outputSwitching"
      :fullscreen-supported="fullscreenSupported"
      @action="handleControlAction"
      @visibility-change="handleControlsVisibilityChange"
    />
  </section>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { GmvPlayerCore } from '../core/GmvPlayerCore';
import type {
  GmvAiBox,
  GmvDeviceStatus,
  GmvMediaMode,
  GmvOsdItem,
  GmvPlayerControlAction,
  GmvPlayerControlsConfig,
  GmvPlayerControlsState,
  GmvPlayerOutputOption,
  GmvPtzCommand,
  GmvSource,
  GmvViewCapabilities,
} from '../core/types';
import PlayerControls from './PlayerControls.vue';

const defaultControls: GmvPlayerControlsConfig = {
  items: ['play', 'audio', 'snapshot', 'info', 'fullscreen', 'ptz', 'record', 'talk', 'streamSwitch', 'playbackRate', 'timeline', 'presets'],
  visibility: 'auto',
  autoHideDelayMs: 3000,
  playbackRates: [0.5, 1, 2, 4],
};

const props = withDefaults(
  defineProps<{
    sources: GmvSource[];
    deviceId?: string;
    channelId?: string;
    title?: string;
    status?: GmvDeviceStatus;
    viewers?: number;
    mediaMode?: GmvMediaMode;
    streamId?: string;
    mediaNodeId?: string;
    sessionNodeId?: string;
    audioCodec?: string;
    poster?: string;
    osd?: GmvOsdItem[];
    aiBoxes?: GmvAiBox[];
    capabilities?: GmvViewCapabilities;
    controls?: GmvPlayerControlsConfig;
    outputType?: string;
    outputOptions?: GmvPlayerOutputOption[];
    outputSwitching?: boolean;
    startupText?: string;
    startupCanCancel?: boolean;
    playbackDurationMs?: number;
    playbackStartTimeMs?: number;
    playbackEndTimeMs?: number;
    /** @deprecated 请使用 controls.visibility。 */
    controlsVisible?: boolean;
  }>(),
  {
    status: 'idle',
    mediaMode: 'live',
    osd: () => [],
    aiBoxes: () => [],
    capabilities: () => ({}),
    outputOptions: () => [],
    controlsVisible: undefined,
  },
);

const emit = defineEmits<{
  snapshot: [{ deviceId?: string; channelId?: string; fileName: string }];
  snapshotError: [{ message: string }];
  recordStart: [{ deviceId?: string; channelId?: string }];
  recordStop: [{ deviceId?: string; channelId?: string }];
  cloudRecordRequest: [];
  cloudRecordCreate: [{ startTimeMs: number; endTimeMs: number }];
  ptz: [GmvPtzCommand];
  presetCall: [{ presetId: string }];
  presetSet: [{ presetId: string }];
  talkStart: [];
  talkStop: [];
  playbackSeek: [{ timeMs: number }];
  playbackRateChange: [{ rate: number }];
  playbackStateChange: [{ paused: boolean }];
  playbackProgress: [{ mediaTimeMs: number }];
  streamSwitch: [{ source: GmvSource }];
  outputTypeChange: [outputType: string];
  playing: [{ source?: GmvSource }];
  playbackError: [{ message: string; source?: GmvSource }];
  playbackSwitchCancel: [];
  reconnect: [];
}>();

const playerRef = ref<HTMLElement>();
const videoARef = ref<HTMLVideoElement>();
const videoBRef = ref<HTMLVideoElement>();
const controlsRef = ref<InstanceType<typeof PlayerControls>>();
type VideoSlot = 0 | 1;
const activeVideoSlot = ref<VideoSlot>(0);
const activePlaybackReady = ref(false);
const players: Array<GmvPlayerCore | undefined> = [undefined, undefined];
const playerStops: Array<Array<() => void>> = [[], []];
let lastFrameProgressAt = 0;
let metricWindowFrames = 0;
const frameProgressHandles: Array<number | undefined> = [undefined, undefined];

function scheduleFrameProgress(slot: VideoSlot) {
  const video = videoForSlot(slot);
  if (!video || typeof video.requestVideoFrameCallback !== 'function') return;
  frameProgressHandles[slot] = video.requestVideoFrameCallback((now, metadata) => {
    if (slot === activeVideoSlot.value) {
      metricWindowFrames += 1;
      mediaStats.currentMediaTimeMs = metadata.mediaTime * 1000;
      if (lastFrameProgressAt === 0) {
        lastFrameProgressAt = now;
      } else if (now - lastFrameProgressAt >= 1000) {
        const elapsedMs = Math.max(1, now - lastFrameProgressAt);
        mediaStats.fps = metricWindowFrames * 1000 / elapsedMs;
        metricWindowFrames = 0;
        lastFrameProgressAt = now;
        sampleMediaStats(video);
        emit('playbackProgress', { mediaTimeMs: metadata.mediaTime * 1000 });
      }
    }
    scheduleFrameProgress(slot);
  });
}

function stopFrameProgress() {
  ([0, 1] as VideoSlot[]).forEach((slot) => {
    const video = videoForSlot(slot);
    const handle = frameProgressHandles[slot];
    if (video && handle !== undefined && typeof video.cancelVideoFrameCallback === 'function') {
      video.cancelVideoFrameCallback(handle);
    }
    frameProgressHandles[slot] = undefined;
  });
}

let playerLoadVersion = 0;
const viewState = ref<GmvDeviceStatus>('idle');
const isLoading = ref(false);
const lastError = ref('');
const startupProgress = ref<{ elapsedMs: number; checkpointMs: number; hardTimeoutMs: number }>();
const isFullscreen = ref(false);
const infoOpen = ref(false);
const ptzOpen = ref(false);
const recording = ref(false);
const talking = ref(false);
const audioEnabled = ref(false);
const ptzSpeed = ref(64);
const playbackRate = ref(1);
const seekMs = ref(0);
const selectedSourceUrl = ref('');
const activeSource = ref<GmvSource>();
const controlsAreVisible = ref(true);
const mediaStats = reactive({
  width: 0,
  height: 0,
  totalFrames: 0,
  droppedFrames: 0,
  fps: 0,
  bufferSeconds: 0,
  currentMediaTimeMs: 0,
});

const basePayload = computed(() => ({ deviceId: props.deviceId, channelId: props.channelId }));
const fullscreenSupported = computed(() => typeof document !== 'undefined' && !!document.fullscreenEnabled);
const effectiveControls = computed<GmvPlayerControlsConfig>(() => ({
  ...defaultControls,
  ...props.controls,
  items: props.controls?.items ?? defaultControls.items,
  overflowItems: props.controls?.overflowItems ?? defaultControls.overflowItems,
  visibility: props.controlsVisible === undefined
    ? props.controls?.visibility ?? defaultControls.visibility
    : props.controlsVisible ? 'always' : 'hidden',
}));
const controlsState = computed<GmvPlayerControlsState>(() => ({
  playbackState: viewState.value,
  audioEnabled: audioEnabled.value,
  fullscreen: isFullscreen.value,
  infoOpen: infoOpen.value,
  ptzOpen: ptzOpen.value,
  recording: recording.value,
  talking: talking.value,
  playbackRate: playbackRate.value,
  seekMs: seekMs.value,
  durationMs: props.playbackDurationMs ?? 86_400_000,
  timelineStartTimeMs: props.playbackStartTimeMs,
  timelineEndTimeMs: props.playbackEndTimeMs,
  selectedSourceUrl: selectedSourceUrl.value,
  selectedOutputType: props.outputType ?? '',
}));
const statusLabel = computed(() => {
  if (viewState.value === 'playing') return '播放中';
  if (viewState.value === 'reconnecting') return '重连中';
  if (viewState.value === 'error') return '异常';
  if (activePlaybackReady.value) return '已暂停';
  if (props.status === 'online') return '在线';
  if (props.status === 'offline') return '离线';
  return '待播放';
});
const currentSource = computed(() => activeSource.value ?? props.sources[0]);
const mediaModeLabel = computed(() => props.mediaMode === 'playback' ? '历史回放' : '实时直播');
const outputFormatLabel = computed(() => {
  const configured = props.outputOptions.find((option) => option.value === props.outputType)?.label;
  if (configured) return configured;
  return formatProtocol(currentSource.value?.protocol);
});
const resolutionText = computed(() => mediaStats.width && mediaStats.height ? `${mediaStats.width}×${mediaStats.height}` : '-');
const videoInfoText = computed(() => [formatVideoCodec(currentSource.value?.codec), resolutionText.value].filter((item) => item !== '-').join(' · ') || '-');
const audioInfoText = computed(() => {
  if (currentSource.value?.hasAudio === false) return '无音频';
  if (props.audioCodec) return props.audioCodec.toUpperCase();
  return currentSource.value?.hasAudio ? '有音频' : '-';
});
const statusViewersText = computed(() => props.viewers === undefined ? statusLabel.value : `${statusLabel.value} · ${props.viewers} 人观看`);
const playbackRangeText = computed(() => {
  if (props.playbackStartTimeMs === undefined || props.playbackEndTimeMs === undefined) return '-';
  return `${formatDateTime(props.playbackStartTimeMs)} 至 ${formatDateTime(props.playbackEndTimeMs)}`;
});
const playbackPositionText = computed(() => {
  const positionMs = activePlaybackReady.value ? mediaStats.currentMediaTimeMs : seekMs.value;
  return props.playbackStartTimeMs === undefined
    ? formatDuration(positionMs)
    : formatDateTime(props.playbackStartTimeMs + positionMs);
});
const droppedFramesText = computed(() => {
  if (!mediaStats.totalFrames) return '-';
  const ratio = mediaStats.droppedFrames / mediaStats.totalFrames * 100;
  return `${mediaStats.droppedFrames}（${ratio.toFixed(2)}%）`;
});
const outputStatusLabel = computed(() => {
  if (props.outputSwitching) return '切换中';
  if (viewState.value === 'error') return '异常';
  if (viewState.value === 'playing') return '输出中';
  if (activePlaybackReady.value) return '已暂停';
  return '待输出';
});
const startupProgressText = computed(() => {
  const progress = startupProgress.value;
  if (!progress) return '';
  if (progress.elapsedMs < progress.checkpointMs) {
    const remaining = Math.max(1, Math.ceil((progress.checkpointMs - progress.elapsedMs) / 1_000));
    return `播放器正在缓冲，${remaining} 秒后检查启动结果`;
  }
  const remaining = Math.max(0, Math.ceil((progress.hardTimeoutMs - progress.elapsedMs) / 1_000));
  return `播放器仍在缓冲，继续等待中（最多 ${remaining} 秒）`;
});

function sampleMediaStats(video: HTMLVideoElement) {
  mediaStats.width = video.videoWidth;
  mediaStats.height = video.videoHeight;
  mediaStats.currentMediaTimeMs = video.currentTime * 1000;
  const quality = typeof video.getVideoPlaybackQuality === 'function' ? video.getVideoPlaybackQuality() : undefined;
  mediaStats.totalFrames = quality?.totalVideoFrames ?? 0;
  mediaStats.droppedFrames = quality?.droppedVideoFrames ?? 0;
  mediaStats.bufferSeconds = 0;
  for (let index = 0; index < video.buffered.length; index += 1) {
    const start = video.buffered.start(index);
    const end = video.buffered.end(index);
    if (start <= video.currentTime && end >= video.currentTime) {
      mediaStats.bufferSeconds = Math.max(0, end - video.currentTime);
      break;
    }
  }
}

function resetMediaStats() {
  mediaStats.width = 0;
  mediaStats.height = 0;
  mediaStats.totalFrames = 0;
  mediaStats.droppedFrames = 0;
  mediaStats.fps = 0;
  mediaStats.bufferSeconds = 0;
  mediaStats.currentMediaTimeMs = 0;
  metricWindowFrames = 0;
  lastFrameProgressAt = 0;
}

function formatProtocol(protocol?: GmvSource['protocol']) {
  if (!protocol) return '-';
  return ({ flv: 'HTTP-FLV', hls: 'HLS-fMP4', fmp4: 'HTTP-fMP4', mp4: 'MP4' } as const)[protocol];
}

function formatVideoCodec(codec?: GmvSource['codec']) {
  if (codec === 'h264') return 'H.264';
  if (codec === 'h265') return 'H.265';
  return '-';
}

function twoDigits(value: number) {
  return String(value).padStart(2, '0');
}

function formatDateTime(timeMs: number) {
  const time = new Date(timeMs);
  return `${time.getFullYear()}-${twoDigits(time.getMonth() + 1)}-${twoDigits(time.getDate())} ${twoDigits(time.getHours())}:${twoDigits(time.getMinutes())}:${twoDigits(time.getSeconds())}`;
}

function formatDuration(timeMs: number) {
  const totalSeconds = Math.max(0, Math.floor(timeMs / 1000));
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor(totalSeconds % 3600 / 60);
  const seconds = totalSeconds % 60;
  return `${twoDigits(hours)}:${twoDigits(minutes)}:${twoDigits(seconds)}`;
}

onMounted(() => {
  scheduleFrameProgress(0);
  scheduleFrameProgress(1);
  document.addEventListener('fullscreenchange', updateFullscreenState);
  void mountPlayer();
});

onBeforeUnmount(() => {
  stopFrameProgress();
  document.removeEventListener('fullscreenchange', updateFullscreenState);
  destroyPlayer();
});

watch(
  () => props.sources,
  () => {
    void mountPlayer();
  },
  { deep: true },
);

watch(() => props.capabilities.ptz, (value) => {
  if (value === false) closePtzPanel();
});

async function mountPlayer(sources = props.sources) {
  const version = ++playerLoadVersion;
  if (sources.length === 0) {
    destroyPlayer();
    return;
  }
  if (activePlaybackReady.value && activeSource.value?.url === sources[0].url) {
    destroyPlayerSlot(activeVideoSlot.value === 0 ? 1 : 0);
    viewState.value = 'playing';
    isLoading.value = false;
    startupProgress.value = undefined;
    return;
  }
  const hasActivePlayback = activePlaybackReady.value && !!players[activeVideoSlot.value];
  const retainedSlot = activeVideoSlot.value;
  const retainedSource = activeSource.value;
  const retainedSourceUrl = selectedSourceUrl.value;
  const slot: VideoSlot = hasActivePlayback ? (activeVideoSlot.value === 0 ? 1 : 0) : activeVideoSlot.value;
  const video = videoForSlot(slot);
  if (!video) return;
  destroyPlayerSlot(slot);

  let slotSource = sources[0];
  selectedSourceUrl.value = hasActivePlayback ? selectedSourceUrl.value : sources[0].url;
  const core = new GmvPlayerCore({
    video,
    sources,
    autoplay: true,
    muted: !audioEnabled.value,
    fallback: true,
  });
  players[slot] = core;

  playerStops[slot].push(core.on('loading', () => {
    if (version !== playerLoadVersion) return;
    viewState.value = hasActivePlayback ? 'reconnecting' : 'idle';
    isLoading.value = true;
    lastError.value = '';
    startupProgress.value = undefined;
  }));
  playerStops[slot].push(core.on('startupProgress', (progress) => {
    if (version === playerLoadVersion) startupProgress.value = progress;
  }));
  playerStops[slot].push(core.on('playing', () => {
    if (version !== playerLoadVersion) return;
    const previousSlot = activeVideoSlot.value;
    const changed = activeSource.value?.url !== slotSource.url;
    activeVideoSlot.value = slot;
    activePlaybackReady.value = true;
    activeSource.value = slotSource;
    selectedSourceUrl.value = slotSource.url;
    viewState.value = 'playing';
    isLoading.value = false;
    lastError.value = '';
    startupProgress.value = undefined;
    if (changed) {
      resetMediaStats();
      playbackRate.value = 1;
      video.playbackRate = 1;
    }
    sampleMediaStats(video);
    void nextTick().then(() => {
      if (version !== playerLoadVersion || activeVideoSlot.value !== slot || players[slot] !== core) return;
      if (previousSlot !== slot) destroyPlayerSlot(previousSlot);
      emit('playing', { source: slotSource });
    });
  }));
  playerStops[slot].push(core.on('paused', () => {
    if (slot === activeVideoSlot.value) {
      viewState.value = 'idle';
      isLoading.value = false;
    }
  }));
  playerStops[slot].push(core.on('reconnecting', () => {
    if (version === playerLoadVersion && !hasActivePlayback) {
      viewState.value = 'reconnecting';
      isLoading.value = true;
    }
  }));
  playerStops[slot].push(core.on('sourceChanged', ({ source }) => {
    slotSource = source;
  }));
  playerStops[slot].push(core.on('error', ({ message }) => {
    if (version !== playerLoadVersion) return;
    destroyPlayerSlot(slot);
    isLoading.value = false;
    startupProgress.value = undefined;
    if (hasActivePlayback && retainedSlot !== slot && players[retainedSlot]) {
      activeVideoSlot.value = retainedSlot;
      activePlaybackReady.value = true;
      activeSource.value = retainedSource;
      selectedSourceUrl.value = retainedSourceUrl;
      viewState.value = 'playing';
      lastError.value = '';
    } else {
      activePlaybackReady.value = false;
      activeSource.value = undefined;
      viewState.value = 'error';
      lastError.value = message;
    }
    emit('playbackError', { message, source: slotSource });
  }));

  await core.load();
}

function destroyPlayer() {
  playerLoadVersion += 1;
  destroyPlayerSlot(0);
  destroyPlayerSlot(1);
  activeSource.value = undefined;
  activePlaybackReady.value = false;
  isLoading.value = false;
  startupProgress.value = undefined;
  resetMediaStats();
}

function destroyPlayerSlot(slot: VideoSlot) {
  while (playerStops[slot].length) playerStops[slot].pop()?.();
  players[slot]?.destroy();
  players[slot] = undefined;
}

function videoForSlot(slot: VideoSlot) {
  return slot === 0 ? videoARef.value : videoBRef.value;
}

function activePlayer() {
  return players[activeVideoSlot.value];
}

function activeVideo() {
  return videoForSlot(activeVideoSlot.value);
}

function togglePlay() {
  const mode = activeSource.value?.rateMode ?? 'disabled';
  if (mode === 'remote-stream') {
    emit('playbackStateChange', { paused: viewState.value === 'playing' });
    return;
  }
  if (viewState.value === 'playing') {
    activePlayer()?.pause();
    return;
  }
  void activePlayer()?.play();
}

function toggleRecord() {
  recording.value = !recording.value;
  if (recording.value) {
    emit('recordStart', basePayload.value);
  } else {
    emit('recordStop', basePayload.value);
  }
}

function toggleAudio() {
  if (props.capabilities.audio === false) return;
  audioEnabled.value = !audioEnabled.value;
  if (activeVideo()) activeVideo()!.muted = !audioEnabled.value;
}

function captureSnapshot() {
  const video = activeVideo();
  if (!video || video.videoWidth <= 0 || video.videoHeight <= 0) {
    emit('snapshotError', { message: '当前播放画面尚未就绪，无法截图' });
    return;
  }
  const canvas = document.createElement('canvas');
  canvas.width = video.videoWidth;
  canvas.height = video.videoHeight;
  const context = canvas.getContext('2d');
  if (!context) {
    emit('snapshotError', { message: '当前浏览器不支持画面截图' });
    return;
  }
  try {
    context.drawImage(video, 0, 0, canvas.width, canvas.height);
    const fileName = snapshotFileName();
    const link = document.createElement('a');
    link.href = canvas.toDataURL('image/png');
    link.download = fileName;
    link.style.display = 'none';
    document.body.appendChild(link);
    try {
      link.click();
    } finally {
      link.remove();
    }
    emit('snapshot', { ...basePayload.value, fileName });
  } catch {
    emit('snapshotError', { message: '画面截图失败，请检查媒体服务的跨域配置' });
  }
}

function snapshotFileName() {
  const name = (props.title || props.channelId || 'gmv').replace(/[\\/:*?"<>|]+/g, '-');
  const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
  return `${name}-${timestamp}.png`;
}

function toggleTalk() {
  talking.value = !talking.value;
  if (talking.value) {
    emit('talkStart');
  } else {
    emit('talkStop');
  }
}

function ptz(action: GmvPtzCommand['action']) {
  if (props.capabilities.ptz === false) return;
  emit('ptz', { action, speed: ptzSpeed.value });
}

function ptzStop() {
  if (props.capabilities.ptz === false) return;
  emit('ptz', { action: 'stop', speed: ptzSpeed.value });
}

function switchSource(url: string) {
  const source = props.sources.find((item) => item.url === url);
  if (!source) return;
  selectedSourceUrl.value = url;
  emit('streamSwitch', { source });
  void mountPlayer([source]);
}

function setPlaybackRate(rate: number) {
  const mode = activeSource.value?.rateMode ?? (activeSource.value?.protocol === 'mp4' ? 'local-file' : 'disabled');
  if (mode === 'disabled') return;
  if (mode === 'local-file') {
    playbackRate.value = rate;
    if (activeVideo()) activeVideo()!.playbackRate = rate;
    return;
  }
  emit('playbackRateChange', { rate });
}

function confirmPlaybackRate(rate: number) {
  playbackRate.value = rate;
  if (activeVideo()) activeVideo()!.playbackRate = rate;
}

function confirmPlaybackState(paused: boolean) {
  if (paused) activePlayer()?.pause();
  else void activePlayer()?.play();
}

function confirmPlaybackProgress(timeMs: number) {
  seekMs.value = Math.max(0, timeMs);
}

defineExpose({ confirmPlaybackRate, confirmPlaybackState, confirmPlaybackProgress });

async function toggleFullscreen() {
  const element = playerRef.value;
  if (!element || !fullscreenSupported.value) return;

  if (document.fullscreenElement === element) {
    await document.exitFullscreen();
    return;
  }

  await element.requestFullscreen();
}

function updateFullscreenState() {
  isFullscreen.value = !!playerRef.value && document.fullscreenElement === playerRef.value;
}

function reconnect() {
  emit('reconnect');
  void activePlayer()?.reconnect();
}

function cancelPendingPlaybackSwitch() {
  if (!activePlaybackReady.value || (!isLoading.value && !props.outputSwitching)) return;
  if (isLoading.value) {
    playerLoadVersion += 1;
    destroyPlayerSlot(activeVideoSlot.value === 0 ? 1 : 0);
    viewState.value = 'playing';
    isLoading.value = false;
    startupProgress.value = undefined;
  }
  emit('playbackSwitchCancel');
}

function notifyControlsActivity() {
  controlsRef.value?.notifyActivity();
}

function handlePlayerPointerLeave() {
  controlsRef.value?.notifySurfaceLeave();
}

function setControlsInteraction(active: boolean) {
  controlsRef.value?.setExternalInteractionActive(active);
}

function closePtzPanel() {
  ptzOpen.value = false;
  setControlsInteraction(false);
}

function toggleInfoPanel() {
  infoOpen.value = !infoOpen.value;
}

function togglePtzPanel() {
  if (props.capabilities.ptz === false) return;
  if (ptzOpen.value) {
    closePtzPanel();
    return;
  }
  ptzOpen.value = true;
}

function handleControlsVisibilityChange(visible: boolean) {
  controlsAreVisible.value = visible;
  if (!visible) {
    infoOpen.value = false;
    closePtzPanel();
  }
}

function handleControlAction(action: GmvPlayerControlAction) {
  switch (action.type) {
    case 'play-toggle':
      togglePlay();
      break;
    case 'audio-toggle':
      toggleAudio();
      break;
    case 'snapshot':
      captureSnapshot();
      break;
    case 'output-type-change':
      emit('outputTypeChange', action.outputType);
      break;
    case 'info-toggle':
      toggleInfoPanel();
      break;
    case 'fullscreen-toggle':
      void toggleFullscreen();
      break;
    case 'ptz-toggle':
      togglePtzPanel();
      break;
    case 'record-toggle':
      toggleRecord();
      break;
    case 'cloud-record-request':
      emit('cloudRecordRequest');
      break;
    case 'cloud-record-create':
      emit('cloudRecordCreate', { startTimeMs: action.startTimeMs, endTimeMs: action.endTimeMs });
      break;
    case 'talk-toggle':
      toggleTalk();
      break;
    case 'stream-switch':
      switchSource(action.sourceUrl);
      break;
    case 'rate-change':
      setPlaybackRate(action.rate);
      break;
    case 'seek':
      seekMs.value = action.timeMs;
      emit('playbackSeek', { timeMs: action.timeMs });
      break;
    case 'preset-call':
      emit('presetCall', { presetId: action.presetId });
      break;
    case 'preset-set':
      emit('presetSet', { presetId: action.presetId });
      break;
  }
}

function boxStyle(box: GmvAiBox) {
  return {
    left: box.x + '%',
    top: box.y + '%',
    width: box.width + '%',
    height: box.height + '%',
  };
}
</script>

<style scoped>
.gmv-player {
  position: relative;
  overflow: hidden;
}

.gmv-video {
  position: absolute;
  inset: 0;
  width: 100%;
  height: 100%;
  display: block;
  object-fit: contain;
  background: #02050a;
  visibility: hidden;
}

.gmv-video.is-active {
  visibility: visible;
}

.player-waiting-cover {
  position: absolute;
  inset: 0;
  z-index: 4;
  display: grid;
  place-items: center;
  overflow: hidden;
  background:
    radial-gradient(circle at 50% 48%, rgba(100, 203, 255, .2), transparent 28%),
    linear-gradient(135deg, rgba(4, 15, 25, .95), rgba(1, 6, 12, .98));
}

.waiting-ring {
  position: relative;
  width: 128px;
  height: 128px;
  border: 1px solid rgba(100, 203, 255, .24);
  border-top-color: rgba(100, 203, 255, .86);
  border-radius: 50%;
  animation: waiting-spin 1.35s linear infinite;
  box-shadow: 0 0 32px rgba(100, 203, 255, .2);
}

.waiting-ring::after {
  content: "";
  position: absolute;
  inset: 32px;
  border: 1px solid rgba(37, 211, 102, .24);
  border-right-color: rgba(37, 211, 102, .72);
  border-radius: 50%;
}

.waiting-scan {
  position: absolute;
  inset: 0;
  background: linear-gradient(180deg, transparent 0%, rgba(100, 203, 255, .14) 50%, transparent 100%);
  animation: waiting-scan 1.8s ease-in-out infinite;
}

.startup-progress-text {
  position: absolute;
  left: 16px;
  right: 16px;
  bottom: 18px;
  color: rgba(230, 247, 255, .92);
  font-size: 12px;
  text-align: center;
}

@keyframes waiting-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes waiting-scan {
  from {
    transform: translateY(-100%);
  }

  to {
    transform: translateY(100%);
  }
}
</style>
