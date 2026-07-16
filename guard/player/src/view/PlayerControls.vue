<template>
  <footer
    v-if="renderControls"
    ref="rootRef"
    class="control-bar"
    :class="{ 'is-hidden': !visible }"
    @pointermove.stop="notifyActivity"
    @pointerenter="handlePointerEnter"
    @pointerleave="handlePointerLeave"
    @pointerdown.stop
    @click.stop
    @focusin="handleFocusIn"
    @focusout="handleFocusOut"
  >
    <div class="control-items primary-controls">
      <template v-for="control in primaryItems" :key="control">
        <button
          v-if="control === 'play'"
          type="button"
          aria-label="切换播放状态"
          @click="emitSimple('play-toggle')"
        >
          {{ state.playbackState === "playing" ? "暂停" : "播放" }}
        </button>
        <button
          v-else-if="control === 'audio'"
          type="button"
          :disabled="capabilities.audio === false"
          aria-label="切换声音"
          @click="emitSimple('audio-toggle')"
        >
          {{ state.audioEnabled ? "静音" : "声音" }}
        </button>
        <button
          v-else-if="control === 'snapshot'"
          type="button"
          :disabled="capabilities.snapshot === false"
          aria-label="截图"
          @click="emitSimple('snapshot')"
        >
          截图
        </button>
        <button
          v-else-if="control === 'fullscreen'"
          type="button"
          :disabled="!fullscreenSupported"
          aria-label="切换全屏"
          @click="emitSimple('fullscreen-toggle')"
        >
          {{ state.fullscreen ? "退出全屏" : "全屏" }}
        </button>
        <button
          v-else-if="control === 'ptz'"
          type="button"
          :class="{ active: state.ptzOpen }"
          :disabled="capabilities.ptz === false"
          aria-label="切换云台控制"
          :aria-expanded="state.ptzOpen"
          @click="emitSimple('ptz-toggle')"
        >
          云台
        </button>
        <button
          v-else-if="control === 'record'"
          type="button"
          :disabled="capabilities.record === false"
          aria-label="切换录像状态"
          @click="emitSimple('record-toggle')"
        >
          {{ state.recording ? "停录像" : "录像" }}
        </button>
        <button
          v-else-if="control === 'talk'"
          type="button"
          :disabled="capabilities.talk === false"
          aria-label="切换对讲状态"
          @click="emitSimple('talk-toggle')"
        >
          {{ state.talking ? "停对讲" : "对讲" }}
        </button>
        <select
          v-else-if="control === 'streamSwitch'"
          :value="state.selectedSourceUrl"
          :disabled="capabilities.streamSwitch === false"
          aria-label="切换码流"
          @change="emitSourceChange($event)"
        >
          <option v-for="source in sources" :key="source.url" :value="source.url">
            {{ source.label || source.protocol + ":" + (source.codec || "auto") }}
          </option>
        </select>
        <select
          v-else-if="control === 'playbackRate'"
          :value="state.playbackRate"
          :disabled="capabilities.playback === false"
          aria-label="播放倍速"
          @change="emitRateChange($event)"
        >
          <option v-for="rate in playbackRates" :key="rate" :value="rate">{{ rate }}x</option>
        </select>
        <label
          v-else-if="control === 'timeline'"
          class="timeline"
          :class="{ disabled: capabilities.playback === false }"
        >
          <span>回放</span>
          <input
            :value="state.seekMs"
            type="range"
            min="0"
            :max="state.durationMs"
            step="1000"
            :disabled="capabilities.playback === false"
            aria-label="回放进度"
            @pointerdown="beginInteraction"
            @pointerup="endInteraction"
            @pointercancel="endInteraction"
            @change="emitSeek($event)"
          />
        </label>
        <div v-else-if="control === 'presets'" class="preset-box">
          <input
            v-model="presetId"
            :disabled="capabilities.presets === false"
            aria-label="预置点编号"
            placeholder="预置点"
          />
          <button
            type="button"
            :disabled="capabilities.presets === false"
            @click="emitPreset('preset-call')"
          >
            调用
          </button>
          <button
            type="button"
            :disabled="capabilities.presets === false"
            @click="emitPreset('preset-set')"
          >
            设置
          </button>
        </div>
      </template>

      <button
        v-if="overflowItems.length"
        ref="moreButtonRef"
        type="button"
        class="more-button"
        aria-label="更多操作"
        aria-haspopup="true"
        :aria-expanded="overflowOpen"
        @click="toggleOverflow"
      >
        …
      </button>
    </div>

    <div v-if="overflowOpen" class="overflow-menu" aria-label="更多播放器操作">
      <template v-for="control in overflowItems" :key="control">
        <button
          v-if="control === 'play'"
          type="button"
          aria-label="切换播放状态"
          @click="emitSimple('play-toggle', true)"
        >
          {{ state.playbackState === "playing" ? "暂停" : "播放" }}
        </button>
        <button
          v-else-if="control === 'audio'"
          type="button"
          :disabled="capabilities.audio === false"
          aria-label="切换声音"
          @click="emitSimple('audio-toggle', true)"
        >
          {{ state.audioEnabled ? "静音" : "声音" }}
        </button>
        <button
          v-else-if="control === 'snapshot'"
          type="button"
          :disabled="capabilities.snapshot === false"
          aria-label="截图"
          @click="emitSimple('snapshot', true)"
        >
          截图
        </button>
        <button
          v-else-if="control === 'fullscreen'"
          type="button"
          :disabled="!fullscreenSupported"
          aria-label="切换全屏"
          @click="emitSimple('fullscreen-toggle', true)"
        >
          {{ state.fullscreen ? "退出全屏" : "全屏" }}
        </button>
        <button
          v-else-if="control === 'ptz'"
          type="button"
          :class="{ active: state.ptzOpen }"
          :disabled="capabilities.ptz === false"
          aria-label="切换云台控制"
          :aria-expanded="state.ptzOpen"
          @click="emitSimple('ptz-toggle', true)"
        >
          云台
        </button>
        <button
          v-else-if="control === 'record'"
          type="button"
          :disabled="capabilities.record === false"
          aria-label="切换录像状态"
          @click="emitSimple('record-toggle', true)"
        >
          {{ state.recording ? "停录像" : "录像" }}
        </button>
        <button
          v-else-if="control === 'talk'"
          type="button"
          :disabled="capabilities.talk === false"
          aria-label="切换对讲状态"
          @click="emitSimple('talk-toggle', true)"
        >
          {{ state.talking ? "停对讲" : "对讲" }}
        </button>
        <select
          v-else-if="control === 'streamSwitch'"
          :value="state.selectedSourceUrl"
          :disabled="capabilities.streamSwitch === false"
          aria-label="切换码流"
          @change="emitSourceChange($event, true)"
        >
          <option v-for="source in sources" :key="source.url" :value="source.url">
            {{ source.label || source.protocol + ":" + (source.codec || "auto") }}
          </option>
        </select>
        <select
          v-else-if="control === 'playbackRate'"
          :value="state.playbackRate"
          :disabled="capabilities.playback === false"
          aria-label="播放倍速"
          @change="emitRateChange($event, true)"
        >
          <option v-for="rate in playbackRates" :key="rate" :value="rate">{{ rate }}x</option>
        </select>
        <label
          v-else-if="control === 'timeline'"
          class="timeline"
          :class="{ disabled: capabilities.playback === false }"
        >
          <span>回放</span>
          <input
            :value="state.seekMs"
            type="range"
            min="0"
            :max="state.durationMs"
            step="1000"
            :disabled="capabilities.playback === false"
            aria-label="回放进度"
            @pointerdown="beginInteraction"
            @pointerup="endInteraction"
            @pointercancel="endInteraction"
            @change="emitSeek($event, true)"
          />
        </label>
        <div v-else-if="control === 'presets'" class="preset-box">
          <input
            v-model="presetId"
            :disabled="capabilities.presets === false"
            aria-label="预置点编号"
            placeholder="预置点"
          />
          <button
            type="button"
            :disabled="capabilities.presets === false"
            @click="emitPreset('preset-call', true)"
          >
            调用
          </button>
          <button
            type="button"
            :disabled="capabilities.presets === false"
            @click="emitPreset('preset-set', true)"
          >
            设置
          </button>
        </div>
      </template>
    </div>
  </footer>
</template>

<script setup lang="ts">
import { computed, nextTick, onBeforeUnmount, onMounted, ref, watch } from "vue";
import type {
  GmvPlayerControlAction,
  GmvPlayerControlsConfig,
  GmvPlayerControlsState,
  GmvSource,
  GmvViewCapabilities,
} from "../core/types";

const props = withDefaults(
  defineProps<{
    config: GmvPlayerControlsConfig;
    state: GmvPlayerControlsState;
    capabilities?: GmvViewCapabilities;
    sources?: GmvSource[];
    fullscreenSupported?: boolean;
  }>(),
  {
    capabilities: () => ({}),
    sources: () => [],
    fullscreenSupported: false,
  },
);

const emit = defineEmits<{
  action: [GmvPlayerControlAction];
  visibilityChange: [visible: boolean];
}>();

const rootRef = ref<HTMLElement>();
const moreButtonRef = ref<HTMLButtonElement>();
const visible = ref(true);
const overflowOpen = ref(false);
const pointerInside = ref(false);
const focusWithin = ref(false);
const interactionActive = ref(false);
const presetId = ref("1");
let hideTimer: number | undefined;

const visibility = computed(() => props.config.visibility ?? "auto");
const renderControls = computed(() => visibility.value !== "hidden");
const playbackRates = computed(() =>
  props.config.playbackRates?.length ? props.config.playbackRates : [0.5, 1, 2, 4],
);
const primaryItems = computed(() => [...new Set(props.config.items)]);
const overflowItems = computed(() => {
  const primary = new Set(primaryItems.value);
  return [...new Set(props.config.overflowItems ?? [])].filter((item) => !primary.has(item));
});
const canAutoHide = computed(
  () =>
    visibility.value === "auto" &&
    props.state.playbackState === "playing" &&
    !interactionActive.value,
);

watch(visible, (value) => emit("visibilityChange", value), { immediate: true });
watch(
  () => [visibility.value, props.state.playbackState, props.config.autoHideDelayMs] as const,
  syncVisibility,
  { immediate: true },
);
watch(interactionActive, () => {
  if (canAutoHide.value) scheduleHide();
  else clearHideTimer();
});
watch(overflowItems, (items) => {
  if (!items.length) closeOverflow(false);
});

onMounted(() => {
  document.addEventListener("pointerdown", handleDocumentPointerDown);
  document.addEventListener("keydown", handleDocumentKeyDown);
});

onBeforeUnmount(() => {
  clearHideTimer();
  document.removeEventListener("pointerdown", handleDocumentPointerDown);
  document.removeEventListener("keydown", handleDocumentKeyDown);
});

function syncVisibility() {
  clearHideTimer();
  if (visibility.value === "hidden") {
    visible.value = false;
    overflowOpen.value = false;
    return;
  }
  visible.value = true;
  if (canAutoHide.value) scheduleHide();
}

function clearHideTimer() {
  if (hideTimer === undefined) return;
  window.clearTimeout(hideTimer);
  hideTimer = undefined;
}

function scheduleHide() {
  clearHideTimer();
  if (!canAutoHide.value) return;
  hideTimer = window.setTimeout(() => {
    hideTimer = undefined;
    visible.value = false;
    overflowOpen.value = false;
  }, props.config.autoHideDelayMs ?? 3000);
}

function notifyActivity() {
  if (visibility.value === "hidden") return;
  visible.value = true;
  if (canAutoHide.value) scheduleHide();
}

function notifySurfaceLeave() {
  pointerInside.value = false;
  focusWithin.value = false;
  interactionActive.value = false;
  overflowOpen.value = false;
  if (canAutoHide.value) scheduleHide();
}

function setExternalInteractionActive(active: boolean) {
  interactionActive.value = active;
  if (active) notifyActivity();
  else if (canAutoHide.value) scheduleHide();
}

function handlePointerEnter() {
  pointerInside.value = true;
  notifyActivity();
}

function handlePointerLeave() {
  pointerInside.value = false;
  if (canAutoHide.value) scheduleHide();
}

function handleFocusIn() {
  focusWithin.value = true;
  notifyActivity();
}

function handleFocusOut(event: FocusEvent) {
  const next = event.relatedTarget;
  if (next instanceof Node && rootRef.value?.contains(next)) return;
  focusWithin.value = false;
  if (canAutoHide.value) scheduleHide();
}

function beginInteraction() {
  interactionActive.value = true;
  notifyActivity();
}

function endInteraction() {
  interactionActive.value = false;
  if (canAutoHide.value) scheduleHide();
}

function toggleOverflow() {
  overflowOpen.value = !overflowOpen.value;
  notifyActivity();
}

function closeOverflow(restoreFocus: boolean) {
  if (!overflowOpen.value) return;
  overflowOpen.value = false;
  if (restoreFocus) void nextTick(() => moreButtonRef.value?.focus());
  if (canAutoHide.value) scheduleHide();
}

function handleDocumentPointerDown(event: PointerEvent) {
  const target = event.target;
  if (!(target instanceof Node) || rootRef.value?.contains(target)) return;
  closeOverflow(false);
}

function handleDocumentKeyDown(event: KeyboardEvent) {
  if (event.key !== "Escape" || !overflowOpen.value) return;
  event.preventDefault();
  closeOverflow(true);
}

function afterAction(fromOverflow: boolean) {
  if (fromOverflow) closeOverflow(false);
  notifyActivity();
}

function emitSimple(
  type: Extract<
    GmvPlayerControlAction,
    {
      type:
        | "play-toggle"
        | "audio-toggle"
        | "snapshot"
        | "fullscreen-toggle"
        | "ptz-toggle"
        | "record-toggle"
        | "talk-toggle";
    }
  >["type"],
  fromOverflow = false,
) {
  emit("action", { type });
  afterAction(fromOverflow);
}

function emitSourceChange(event: Event, fromOverflow = false) {
  emit("action", { type: "stream-switch", sourceUrl: (event.target as HTMLSelectElement).value });
  afterAction(fromOverflow);
}

function emitRateChange(event: Event, fromOverflow = false) {
  emit("action", { type: "rate-change", rate: Number((event.target as HTMLSelectElement).value) });
  afterAction(fromOverflow);
}

function emitSeek(event: Event, fromOverflow = false) {
  emit("action", { type: "seek", timeMs: Number((event.target as HTMLInputElement).value) });
  interactionActive.value = false;
  afterAction(fromOverflow);
}

function emitPreset(type: "preset-call" | "preset-set", fromOverflow = false) {
  emit("action", { type, presetId: presetId.value });
  afterAction(fromOverflow);
}

defineExpose({ notifyActivity, notifySurfaceLeave, setExternalInteractionActive });
</script>

<style scoped>
.control-bar {
  position: absolute;
  inset: 0;
  z-index: 6;
  display: block;
  pointer-events: none;
  opacity: 1;
  visibility: visible;
  transition:
    opacity 180ms ease,
    visibility 180ms ease;
}

.control-bar.is-hidden {
  pointer-events: none;
  opacity: 0;
  visibility: hidden;
}

.control-items {
  display: flex;
  align-items: center;
  gap: 8px;
  min-width: 0;
  overflow-x: auto;
}

.primary-controls {
  position: absolute;
  right: 0;
  bottom: 0;
  left: 0;
  padding: 10px;
  pointer-events: auto;
  background: linear-gradient(0deg, rgba(0, 0, 0, 0.78), transparent);
}

button,
select,
.preset-box input {
  flex: 0 0 auto;
  height: 32px;
  padding: 0 9px;
  border-radius: 5px;
  white-space: nowrap;
}

button.active {
  border-color: var(--accent);
  background: rgba(34, 211, 238, 0.18);
  color: var(--accent);
}

.more-button {
  min-width: 36px;
  font-size: 20px;
  line-height: 1;
}

.timeline {
  display: flex;
  flex: 1 0 130px;
  align-items: center;
  gap: 8px;
  min-width: 130px;
  color: var(--muted);
}

.timeline input {
  width: 100%;
}

.timeline.disabled {
  opacity: 0.45;
}

.preset-box {
  display: flex;
  flex: 0 0 auto;
  gap: 5px;
}

.preset-box input {
  width: 74px;
}

.overflow-menu {
  position: absolute;
  right: 10px;
  bottom: 50px;
  display: grid;
  gap: 7px;
  width: max-content;
  min-width: 132px;
  max-width: calc(100% - 20px);
  max-height: min(360px, calc(100% - 60px));
  padding: 9px;
  overflow: auto;
  pointer-events: auto;
  border: 1px solid var(--line);
  border-radius: 8px;
  background: rgba(3, 10, 24, 0.96);
  box-shadow: 0 14px 36px rgba(0, 0, 0, 0.42);
}

.overflow-menu > button,
.overflow-menu > select {
  width: 100%;
}

.overflow-menu .timeline,
.overflow-menu .preset-box {
  width: min(320px, calc(100vw - 60px));
}

@media (max-width: 1100px) {
  .control-items {
    flex-wrap: nowrap;
  }
}
</style>
