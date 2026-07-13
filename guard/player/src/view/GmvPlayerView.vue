<template>
  <section
    ref="playerRef"
    class="gmv-player"
    :class="['is-' + viewState, { 'player-chrome-hidden': !controlsAreVisible }]"
    @pointermove="notifyControlsActivity"
    @pointerdown="notifyControlsActivity"
    @pointerleave="handlePlayerPointerLeave"
    @keydown="notifyControlsActivity"
  >
    <video ref="videoRef" class="gmv-video" playsinline muted :poster="poster || undefined"></video>

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

    <div v-if="isLoading" class="player-waiting-cover" aria-label="视频加载中" role="status">
      <span class="waiting-ring"></span>
      <span class="waiting-scan"></span>
    </div>

    <header class="player-topbar">
      <div>
        <strong>{{ title || 'GMV Player' }}</strong>
        <span>{{ deviceId || '-' }} / {{ channelId || '-' }}</span>
      </div>
      <div class="status-strip">
        <b>{{ statusLabel }}</b>
        <span>{{ viewers ?? '-' }} 人观看</span>
      </div>
    </header>

    <div v-if="viewState === 'reconnecting' || lastError" class="reconnect-banner">
      <span>{{ lastError || '正在重连...' }}</span>
      <button type="button" @click="reconnect">重连</button>
    </div>

    <div v-if="recording" class="recording-indicator" aria-live="polite">● 录像中</div>

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
      :fullscreen-supported="fullscreenSupported"
      @action="handleControlAction"
      @visibility-change="handleControlsVisibilityChange"
    />
  </section>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { GmvPlayerCore } from '../core/GmvPlayerCore';
import type {
  GmvAiBox,
  GmvDeviceStatus,
  GmvOsdItem,
  GmvPlayerControlAction,
  GmvPlayerControlsConfig,
  GmvPlayerControlsState,
  GmvPtzCommand,
  GmvSource,
  GmvViewCapabilities,
} from '../core/types';
import PlayerControls from './PlayerControls.vue';

const defaultControls: GmvPlayerControlsConfig = {
  items: ['play', 'audio', 'snapshot', 'fullscreen', 'ptz', 'record', 'talk', 'streamSwitch', 'playbackRate', 'timeline', 'presets'],
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
    poster?: string;
    osd?: GmvOsdItem[];
    aiBoxes?: GmvAiBox[];
    capabilities?: GmvViewCapabilities;
    controls?: GmvPlayerControlsConfig;
    /** @deprecated 请使用 controls.visibility。 */
    controlsVisible?: boolean;
  }>(),
  {
    status: 'idle',
    osd: () => [],
    aiBoxes: () => [],
    capabilities: () => ({}),
    controlsVisible: undefined,
  },
);

const emit = defineEmits<{
  snapshot: [{ deviceId?: string; channelId?: string }];
  recordStart: [{ deviceId?: string; channelId?: string }];
  recordStop: [{ deviceId?: string; channelId?: string }];
  ptz: [GmvPtzCommand];
  presetCall: [{ presetId: string }];
  presetSet: [{ presetId: string }];
  talkStart: [];
  talkStop: [];
  playbackSeek: [{ timeMs: number }];
  streamSwitch: [{ source: GmvSource }];
  reconnect: [];
}>();

const playerRef = ref<HTMLElement>();
const videoRef = ref<HTMLVideoElement>();
const player = ref<GmvPlayerCore>();
const controlsRef = ref<InstanceType<typeof PlayerControls>>();
const viewState = ref<GmvDeviceStatus>('idle');
const isLoading = ref(false);
const lastError = ref('');
const isFullscreen = ref(false);
const ptzOpen = ref(false);
const recording = ref(false);
const talking = ref(false);
const audioEnabled = ref(false);
const ptzSpeed = ref(64);
const playbackRate = ref(1);
const seekMs = ref(0);
const selectedSourceUrl = ref('');
const controlsAreVisible = ref(true);
const stops: Array<() => void> = [];

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
  ptzOpen: ptzOpen.value,
  recording: recording.value,
  talking: talking.value,
  playbackRate: playbackRate.value,
  seekMs: seekMs.value,
  selectedSourceUrl: selectedSourceUrl.value,
}));
const statusLabel = computed(() => {
  if (viewState.value === 'playing') return '播放中';
  if (viewState.value === 'reconnecting') return '重连中';
  if (viewState.value === 'error') return '异常';
  if (props.status === 'online') return '在线';
  if (props.status === 'offline') return '离线';
  return '待播放';
});

onMounted(() => {
  document.addEventListener('fullscreenchange', updateFullscreenState);
  void mountPlayer();
});

onBeforeUnmount(() => {
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

async function mountPlayer() {
  destroyPlayer();
  if (!videoRef.value || props.sources.length === 0) return;

  selectedSourceUrl.value = props.sources[0].url;
  const core = new GmvPlayerCore({
    video: videoRef.value,
    sources: props.sources,
    autoplay: true,
    muted: !audioEnabled.value,
    fallback: true,
  });
  player.value = core;

  stops.push(core.on('loading', () => { viewState.value = 'idle'; isLoading.value = true; lastError.value = ''; }));
  stops.push(core.on('playing', () => { viewState.value = 'playing'; isLoading.value = false; lastError.value = ''; }));
  stops.push(core.on('paused', () => { viewState.value = 'idle'; isLoading.value = false; }));
  stops.push(core.on('reconnecting', () => { viewState.value = 'reconnecting'; isLoading.value = true; }));
  stops.push(core.on('sourceChanged', ({ source }) => { selectedSourceUrl.value = source.url; }));
  stops.push(core.on('error', ({ message }) => { viewState.value = 'error'; isLoading.value = false; lastError.value = message; }));

  await core.load();
}

function destroyPlayer() {
  while (stops.length) stops.pop()?.();
  player.value?.destroy();
  player.value = undefined;
  isLoading.value = false;
}

function togglePlay() {
  if (viewState.value === 'playing') {
    player.value?.pause();
    return;
  }
  void player.value?.play();
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
  if (videoRef.value) videoRef.value.muted = !audioEnabled.value;
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
  void player.value?.switchSource(source);
}

function setPlaybackRate(rate: number) {
  playbackRate.value = rate;
  if (videoRef.value) videoRef.value.playbackRate = rate;
}

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
  void player.value?.reconnect();
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
  if (!visible) closePtzPanel();
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
      emit('snapshot', basePayload.value);
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
