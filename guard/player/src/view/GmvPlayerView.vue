<template>
  <section class="gmv-player" :class="'is-' + viewState">
    <video ref="videoRef" class="gmv-video" playsinline muted></video>

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

    <aside class="ptz-panel" v-if="capabilities.ptz !== false">
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

    <footer class="control-bar">
      <button type="button" @click="togglePlay">{{ viewState === 'playing' ? '暂停' : '播放' }}</button>
      <button type="button" :disabled="capabilities.snapshot === false" @click="emit('snapshot', basePayload)">抓拍</button>
      <button type="button" :disabled="capabilities.record === false" @click="toggleRecord">{{ recording ? '停录像' : '录像' }}</button>
      <button type="button" :disabled="capabilities.talk === false" @click="toggleTalk">{{ talking ? '停对讲' : '对讲' }}</button>

      <select :value="selectedSourceUrl" :disabled="capabilities.streamSwitch === false" @change="switchSource">
        <option v-for="source in sources" :key="source.url" :value="source.url">
          {{ source.label || source.protocol + ':' + (source.codec || 'auto') }}
        </option>
      </select>

      <select v-model="playbackRate" :disabled="capabilities.playback === false" @change="setPlaybackRate">
        <option :value="0.5">0.5x</option>
        <option :value="1">1x</option>
        <option :value="2">2x</option>
        <option :value="4">4x</option>
      </select>

      <label class="timeline" :class="{ disabled: capabilities.playback === false }">
        <span>回放</span>
        <input v-model.number="seekMs" type="range" min="0" max="86400000" step="1000" :disabled="capabilities.playback === false" @change="emit('playbackSeek', { timeMs: seekMs })" />
      </label>

      <div class="preset-box" v-if="capabilities.presets !== false">
        <input v-model="presetId" placeholder="预置点" />
        <button type="button" @click="emit('presetCall', { presetId })">调用</button>
        <button type="button" @click="emit('presetSet', { presetId })">设置</button>
      </div>
    </footer>
  </section>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { GmvPlayerCore } from '../core/GmvPlayerCore';
import type { GmvAiBox, GmvDeviceStatus, GmvOsdItem, GmvPtzCommand, GmvSource, GmvViewCapabilities } from '../core/types';

const props = withDefaults(
  defineProps<{
    sources: GmvSource[];
    deviceId?: string;
    channelId?: string;
    title?: string;
    status?: GmvDeviceStatus;
    viewers?: number;
    osd?: GmvOsdItem[];
    aiBoxes?: GmvAiBox[];
    capabilities?: GmvViewCapabilities;
  }>(),
  {
    status: 'idle',
    osd: () => [],
    aiBoxes: () => [],
    capabilities: () => ({}),
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

const videoRef = ref<HTMLVideoElement>();
const player = ref<GmvPlayerCore>();
const viewState = ref<GmvDeviceStatus>('idle');
const lastError = ref('');
const recording = ref(false);
const talking = ref(false);
const ptzSpeed = ref(64);
const playbackRate = ref(1);
const seekMs = ref(0);
const presetId = ref('1');
const selectedSourceUrl = ref('');
const stops: Array<() => void> = [];

const basePayload = computed(() => ({ deviceId: props.deviceId, channelId: props.channelId }));
const statusLabel = computed(() => {
  if (viewState.value === 'playing') return '播放中';
  if (viewState.value === 'reconnecting') return '重连中';
  if (viewState.value === 'error') return '异常';
  if (props.status === 'online') return '在线';
  if (props.status === 'offline') return '离线';
  return '待播放';
});

onMounted(() => {
  void mountPlayer();
});

onBeforeUnmount(() => {
  destroyPlayer();
});

watch(
  () => props.sources,
  () => {
    void mountPlayer();
  },
  { deep: true },
);

async function mountPlayer() {
  destroyPlayer();
  if (!videoRef.value || props.sources.length === 0) return;

  selectedSourceUrl.value = props.sources[0].url;
  const core = new GmvPlayerCore({
    video: videoRef.value,
    sources: props.sources,
    autoplay: true,
    muted: true,
    fallback: true,
  });
  player.value = core;

  stops.push(core.on('loading', () => { viewState.value = 'idle'; lastError.value = ''; }));
  stops.push(core.on('playing', () => { viewState.value = 'playing'; lastError.value = ''; }));
  stops.push(core.on('paused', () => { viewState.value = 'idle'; }));
  stops.push(core.on('reconnecting', () => { viewState.value = 'reconnecting'; }));
  stops.push(core.on('sourceChanged', ({ source }) => { selectedSourceUrl.value = source.url; }));
  stops.push(core.on('error', ({ message }) => { viewState.value = 'error'; lastError.value = message; }));

  await core.load();
}

function destroyPlayer() {
  while (stops.length) stops.pop()?.();
  player.value?.destroy();
  player.value = undefined;
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

function toggleTalk() {
  talking.value = !talking.value;
  if (talking.value) {
    emit('talkStart');
  } else {
    emit('talkStop');
  }
}

function ptz(action: GmvPtzCommand['action']) {
  emit('ptz', { action, speed: ptzSpeed.value });
}

function ptzStop() {
  emit('ptz', { action: 'stop', speed: ptzSpeed.value });
}

function switchSource(event: Event) {
  const url = (event.target as HTMLSelectElement).value;
  const source = props.sources.find((item) => item.url === url);
  if (!source) return;
  selectedSourceUrl.value = url;
  emit('streamSwitch', { source });
  void player.value?.switchSource(source);
}

function setPlaybackRate() {
  if (videoRef.value) videoRef.value.playbackRate = Number(playbackRate.value);
}

function reconnect() {
  emit('reconnect');
  void player.value?.reconnect();
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
