<template>
  <footer
    v-if="renderControls"
    ref="rootRef"
    class="control-bar"
    :class="{ 'is-hidden': !visible, 'has-primary-timeline': hasPrimaryTimeline }"
    @pointermove.stop="notifyActivity"
    @pointerenter="handlePointerEnter"
    @pointerleave="handlePointerLeave"
    @pointerdown.stop
    @click.stop
    @focusin="handleFocusIn"
    @focusout="handleFocusOut"
  >
    <div v-if="hasPrimaryTimeline" class="playback-timeline-row">
      <div class="timeline primary-timeline" :class="{ disabled: capabilities.playback === false }">
        <span class="timeline-boundary">{{ formatTimelineBoundary(timelineStartTimeMs) }}</span>
        <span class="timeline-track">
          <span class="timeline-rail" aria-hidden="true"></span>
          <span
            v-if="hoverTimelineTimeMs !== undefined"
            class="timeline-tooltip"
            :style="{ left: hoverTimelineLeft + '%' }"
          >
            {{ formatTimelineTooltip(hoverTimelineTimeMs) }}
          </span>
          <input
            v-if="timelineMode === 'playback'"
            :value="state.seekMs"
            type="range"
            min="0"
            :max="state.durationMs"
            step="1000"
            :disabled="capabilities.playback === false"
            aria-label="回放进度"
            @pointerdown="beginInteraction"
            @pointermove="updateTimelineHover"
            @pointerleave="clearTimelineHover"
            @pointerup="endInteraction"
            @pointercancel="endInteraction"
            @change="emitSeek($event)"
          />
          <template v-else>
            <span class="clip-selection" :style="clipSelectionStyle" aria-hidden="true"></span>
            <input
              class="clip-range clip-range-a"
              :value="clipHandleAMs"
              type="range"
              min="0"
              :max="state.durationMs"
              step="1000"
              :disabled="capabilities.playback === false"
              aria-label="截取滑块一"
              @pointerdown="beginInteraction"
              @pointermove="updateTimelineHover"
              @pointerleave="clearTimelineHover"
              @pointerup="endInteraction"
              @pointercancel="endInteraction"
              @input="setClipHandleA"
            />
            <input
              class="clip-range clip-range-b"
              :value="clipHandleBMs"
              type="range"
              min="0"
              :max="state.durationMs"
              step="1000"
              :disabled="capabilities.playback === false"
              aria-label="截取滑块二"
              @pointerdown="beginInteraction"
              @pointermove="updateTimelineHover"
              @pointerleave="clearTimelineHover"
              @pointerup="endInteraction"
              @pointercancel="endInteraction"
              @input="setClipHandleB"
            />
          </template>
          <span v-if="timelineTicks.length" class="timeline-ticks" aria-hidden="true">
            <span
              v-for="tick in timelineTicks"
              :key="tick.ratio"
              :style="{ left: tick.ratio * 100 + '%' }"
            >
              {{ formatTimelineTick(tick.timeMs) }}
            </span>
          </span>
        </span>
        <span class="timeline-boundary">{{ formatTimelineBoundary(timelineEndTimeMs) }}</span>
        <span class="timeline-jump primary-timeline-jump">
          <button
            type="button"
            :disabled="capabilities.playback === false || state.seekMs <= 0"
            aria-label="向后跳跃"
            @click="emitJump(-1)"
          >
            后退
          </button>
          <input
            v-model.number="jumpSeconds"
            type="number"
            min="1"
            :max="Math.max(1, Math.floor(state.durationMs / 1000))"
            :disabled="capabilities.playback === false"
            aria-label="跳跃秒数"
          />
          <span>秒</span>
          <button
            type="button"
            :disabled="capabilities.playback === false || state.seekMs >= state.durationMs"
            aria-label="向前跳跃"
            @click="emitJump(1)"
          >
            前进
          </button>
        </span>
      </div>
    </div>
    <div class="control-items primary-controls">
      <template v-for="control in primaryActionItems" :key="control">
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
        <select
          v-else-if="control === 'outputType'"
          :value="state.selectedOutputType"
          :disabled="outputSwitching || !outputOptions.length"
          aria-label="媒体输出格式"
          @change="emitOutputTypeChange($event)"
        >
          <option v-for="option in outputOptions" :key="option.value" :value="option.value">
            {{ option.label }}
          </option>
        </select>
        <button
          v-else-if="control === 'info'"
          type="button"
          :class="{ active: state.infoOpen }"
          aria-label="切换媒体信息"
          :aria-expanded="state.infoOpen"
          @click="emitSimple('info-toggle')"
        >
          信息
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
          v-else-if="control === 'cloudRecord'"
          type="button"
          aria-label="打开下载"
          @click="emitSimple('cloud-record-request')"
        >
          下载
        </button>
        <select
          v-else-if="control === 'streamProfile'"
          :value="state.selectedStreamProfile"
          :disabled="capabilities.streamProfile === false || streamProfileSwitching"
          aria-label="切换主辅码流"
          @change="emitStreamProfileChange($event)"
        >
          <option v-for="option in streamProfileOptions" :key="option.value" :value="option.value">
            {{ option.label }}
          </option>
        </select>
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
        <span v-else-if="control === 'playbackClip'" class="playback-clip-controls">
          <select v-model="timelineMode" aria-label="回放操作模式">
            <option value="playback">回放</option>
            <option value="clip">截取</option>
          </select>
          <span v-if="timelineMode === 'clip'" class="clip-down-tip" :title="clipRangeHint">
            <button
              type="button"
              :disabled="!clipRangeValid || clipRangeLocked || capabilities.playback === false"
              :aria-label="clipRangeLocked ? '截取录像创建中' : '创建截取录像'"
              :aria-busy="clipRangeLocked"
              @click="emitCloudRecordCreate()"
            >
              <span v-if="clipRangeLocked" class="clip-down-spinner" aria-hidden="true"></span>
              <template v-else>DOWN</template>
            </button>
          </span>
        </span>
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
        更多
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
        <select
          v-else-if="control === 'outputType'"
          :value="state.selectedOutputType"
          :disabled="outputSwitching || !outputOptions.length"
          aria-label="媒体输出格式"
          @change="emitOutputTypeChange($event, true)"
        >
          <option v-for="option in outputOptions" :key="option.value" :value="option.value">
            {{ option.label }}
          </option>
        </select>
        <button
          v-else-if="control === 'info'"
          type="button"
          :class="{ active: state.infoOpen }"
          aria-label="切换媒体信息"
          :aria-expanded="state.infoOpen"
          @click="emitSimple('info-toggle', true)"
        >
          信息
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
          v-else-if="control === 'cloudRecord'"
          type="button"
          aria-label="打开下载"
          @click="emitSimple('cloud-record-request', true)"
        >
          下载
        </button>
        <select
          v-else-if="control === 'streamProfile'"
          :value="state.selectedStreamProfile"
          :disabled="capabilities.streamProfile === false || streamProfileSwitching"
          aria-label="切换主辅码流"
          @change="emitStreamProfileChange($event, true)"
        >
          <option v-for="option in streamProfileOptions" :key="option.value" :value="option.value">
            {{ option.label }}
          </option>
        </select>
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
        <span v-else-if="control === 'playbackClip'" class="playback-clip-controls">
          <select v-model="timelineMode" aria-label="回放操作模式">
            <option value="playback">回放</option>
            <option value="clip">截取</option>
          </select>
          <span v-if="timelineMode === 'clip'" class="clip-down-tip" :title="clipRangeHint">
            <button
              type="button"
              :disabled="!clipRangeValid || clipRangeLocked || capabilities.playback === false"
              :aria-label="clipRangeLocked ? '截取录像创建中' : '创建截取录像'"
              :aria-busy="clipRangeLocked"
              @click="emitCloudRecordCreate(true)"
            >
              <span v-if="clipRangeLocked" class="clip-down-spinner" aria-hidden="true"></span>
              <template v-else>DOWN</template>
            </button>
          </span>
        </span>
        <div
          v-else-if="control === 'timeline'"
          class="timeline"
          :class="{ disabled: capabilities.playback === false }"
        >
          <span class="timeline-boundary">{{ formatTimelineBoundary(timelineStartTimeMs) }}</span>
          <span class="timeline-track">
            <span class="timeline-rail" aria-hidden="true"></span>
            <span
              v-if="hoverTimelineTimeMs !== undefined"
              class="timeline-tooltip"
              :style="{ left: hoverTimelineLeft + '%' }"
            >
              {{ formatTimelineTooltip(hoverTimelineTimeMs) }}
            </span>
            <input
              v-if="timelineMode === 'playback'"
              :value="state.seekMs"
              type="range"
              min="0"
              :max="state.durationMs"
              step="1000"
              :disabled="capabilities.playback === false"
              aria-label="回放进度"
              @pointerdown="beginInteraction"
              @pointermove="updateTimelineHover"
              @pointerleave="clearTimelineHover"
              @pointerup="endInteraction"
              @pointercancel="endInteraction"
              @change="emitSeek($event, true)"
            />
            <template v-else>
              <span class="clip-selection" :style="clipSelectionStyle" aria-hidden="true"></span>
              <input
                class="clip-range clip-range-a"
                :value="clipHandleAMs"
                type="range"
                min="0"
                :max="state.durationMs"
                step="1000"
                :disabled="capabilities.playback === false"
                aria-label="截取滑块一"
                @pointerdown="beginInteraction"
                @pointermove="updateTimelineHover"
                @pointerleave="clearTimelineHover"
                @pointerup="endInteraction"
                @pointercancel="endInteraction"
                @input="setClipHandleA"
              />
              <input
                class="clip-range clip-range-b"
                :value="clipHandleBMs"
                type="range"
                min="0"
                :max="state.durationMs"
                step="1000"
                :disabled="capabilities.playback === false"
                aria-label="截取滑块二"
                @pointerdown="beginInteraction"
                @pointermove="updateTimelineHover"
                @pointerleave="clearTimelineHover"
                @pointerup="endInteraction"
                @pointercancel="endInteraction"
                @input="setClipHandleB"
              />
            </template>
            <span v-if="timelineTicks.length" class="timeline-ticks" aria-hidden="true">
              <span
                v-for="tick in timelineTicks"
                :key="tick.ratio"
                :style="{ left: tick.ratio * 100 + '%' }"
              >
                {{ formatTimelineTick(tick.timeMs) }}
              </span>
            </span>
          </span>
          <span class="timeline-boundary">{{ formatTimelineBoundary(timelineEndTimeMs) }}</span>
          <span class="timeline-jump">
            <button
              type="button"
              :disabled="capabilities.playback === false || state.seekMs <= 0"
              aria-label="向后跳跃"
              @click="emitJump(-1, true)"
            >
              后退
            </button>
            <input
              v-model.number="jumpSeconds"
              type="number"
              min="1"
              :max="Math.max(1, Math.floor(state.durationMs / 1000))"
              :disabled="capabilities.playback === false"
              aria-label="跳跃秒数"
            />
            <span>秒</span>
            <button
              type="button"
              :disabled="capabilities.playback === false || state.seekMs >= state.durationMs"
              aria-label="向前跳跃"
              @click="emitJump(1, true)"
            >
              前进
            </button>
          </span>
        </div>
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
  GmvPlayerOutputOption,
  GmvSource,
  GmvStreamProfile,
  GmvStreamProfileOption,
  GmvViewCapabilities,
} from "../core/types";

const props = withDefaults(
  defineProps<{
    config: GmvPlayerControlsConfig;
    state: GmvPlayerControlsState;
    capabilities?: GmvViewCapabilities;
    sources?: GmvSource[];
    outputOptions?: GmvPlayerOutputOption[];
    outputSwitching?: boolean;
    streamProfileOptions?: GmvStreamProfileOption[];
    streamProfileSwitching?: boolean;
    fullscreenSupported?: boolean;
  }>(),
  {
    capabilities: () => ({}),
    sources: () => [],
    outputOptions: () => [],
    outputSwitching: false,
    streamProfileOptions: () => [],
    streamProfileSwitching: false,
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
const jumpSeconds = ref(10);
const timelineMode = ref<"playback" | "clip">("playback");
const clipHandleAMs = ref(0);
const clipHandleBMs = ref(0);
const hoverTimelineTimeMs = ref<number>();
const hoverTimelineLeft = ref(0);
let hideTimer: number | undefined;

const CLIP_MIN_DURATION_MS = 2 * 60 * 1_000;
const CLIP_MAX_DURATION_MS = 2 * 60 * 60 * 1_000;

const visibility = computed(() => props.config.visibility ?? "auto");
const renderControls = computed(() => visibility.value !== "hidden");
const playbackRates = computed(() =>
  props.config.playbackRates?.length ? props.config.playbackRates : [0.5, 1, 2, 4],
);
const timelineStartTimeMs = computed(() => props.state.timelineStartTimeMs);
const timelineEndTimeMs = computed(() => props.state.timelineEndTimeMs);
const clipStartMs = computed(() => Math.min(clipHandleAMs.value, clipHandleBMs.value));
const clipEndMs = computed(() => Math.max(clipHandleAMs.value, clipHandleBMs.value));
const clipDurationMs = computed(() => clipEndMs.value - clipStartMs.value);
const clipRangeValid = computed(() =>
  timelineStartTimeMs.value !== undefined
  && clipDurationMs.value >= CLIP_MIN_DURATION_MS
  && clipDurationMs.value <= CLIP_MAX_DURATION_MS
  && clipEndMs.value <= props.state.durationMs,
);
const clipRangeLocked = computed(() => {
  const start = timelineStartTimeMs.value;
  const locked = props.state.cloudRecordLockedRange;
  if (start === undefined || !locked) return false;
  return locked.startTimeMs === start + clipStartMs.value
    && locked.endTimeMs === start + clipEndMs.value;
});
const clipRangeHint = computed(() => {
  if (timelineStartTimeMs.value === undefined) return "缺少回放时间范围";
  if (clipDurationMs.value < CLIP_MIN_DURATION_MS) return "截取时长不能少于 2 分钟";
  if (clipDurationMs.value > CLIP_MAX_DURATION_MS) return "截取时长不能超过 2 小时";
  if (clipRangeLocked.value) return "该截取时段已提交，请调整开始或结束时间";
  return `${formatTimelineTooltip(timelineStartTimeMs.value! + clipStartMs.value)} 至 ${formatTimelineTooltip(timelineStartTimeMs.value! + clipEndMs.value)}`;
});
const clipSelectionStyle = computed(() => {
  const durationMs = Math.max(1, props.state.durationMs || 0);
  return {
    left: `${clipStartMs.value / durationMs * 100}%`,
    width: `${clipDurationMs.value / durationMs * 100}%`,
  };
});
const timelineTicks = computed(() => {
  const start = timelineStartTimeMs.value;
  const end = timelineEndTimeMs.value;
  if (start === undefined || end === undefined || end <= start) return [];
  return [0.25, 0.5, 0.75].map((ratio) => ({
    ratio,
    timeMs: start + Math.round((end - start) * ratio),
  }));
});
const primaryItems = computed(() => [...new Set(props.config.items)]);
const hasPrimaryTimeline = computed(() => primaryItems.value.includes("timeline"));
const primaryActionItems = computed(() => primaryItems.value.filter((item) => item !== "timeline"));
const overflowItems = computed(() => {
  const primary = new Set(primaryItems.value);
  return [...new Set(props.config.overflowItems ?? [])].filter((item) => !primary.has(item));
});
const canAutoHide = computed(() => visibility.value === "auto" && !interactionActive.value);

watch(visible, (value) => emit("visibilityChange", value), { immediate: true });
watch(
  () => [visibility.value, props.config.autoHideDelayMs] as const,
  syncVisibility,
  { immediate: true },
);
watch(timelineMode, (mode) => {
  if (mode === "clip") resetClipRange();
});
watch(interactionActive, () => {
  if (canAutoHide.value) scheduleHide();
  else clearHideTimer();
});
watch(
  [
    () => props.state.timelineStartTimeMs,
    () => props.state.timelineEndTimeMs,
    () => props.state.durationMs,
  ],
  () => {
    timelineMode.value = "playback";
    resetClipRange();
  },
  { immediate: true },
);
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
        | "info-toggle"
        | "fullscreen-toggle"
        | "ptz-toggle"
        | "record-toggle"
        | "cloud-record-request"
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

function emitStreamProfileChange(event: Event, fromOverflow = false) {
  emit("action", {
    type: "stream-profile-change",
    profile: (event.target as HTMLSelectElement).value as GmvStreamProfile,
  });
  afterAction(fromOverflow);
}

function emitRateChange(event: Event, fromOverflow = false) {
  emit("action", { type: "rate-change", rate: Number((event.target as HTMLSelectElement).value) });
  afterAction(fromOverflow);
}

function resetClipRange() {
  const durationMs = Math.max(0, props.state.durationMs || 0);
  if (durationMs < CLIP_MIN_DURATION_MS) {
    clipHandleAMs.value = 0;
    clipHandleBMs.value = durationMs;
    return;
  }
  const startMs = Math.min(Math.max(0, props.state.seekMs), durationMs - CLIP_MIN_DURATION_MS);
  clipHandleAMs.value = startMs;
  clipHandleBMs.value = startMs + CLIP_MIN_DURATION_MS;
}

function setClipHandleA(event: Event) {
  clipHandleAMs.value = Number((event.target as HTMLInputElement).value);
}

function setClipHandleB(event: Event) {
  clipHandleBMs.value = Number((event.target as HTMLInputElement).value);
}

function emitCloudRecordCreate(fromOverflow = false) {
  const startTimeMs = timelineStartTimeMs.value;
  if (!clipRangeValid.value || startTimeMs === undefined) return;
  emit("action", {
    type: "cloud-record-create",
    startTimeMs: startTimeMs + clipStartMs.value,
    endTimeMs: startTimeMs + clipEndMs.value,
  });
  afterAction(fromOverflow);
}

function emitOutputTypeChange(event: Event, fromOverflow = false) {
  emit("action", { type: "output-type-change", outputType: (event.target as HTMLSelectElement).value });
  afterAction(fromOverflow);
}

function emitSeek(event: Event, fromOverflow = false) {
  emit("action", { type: "seek", timeMs: Number((event.target as HTMLInputElement).value) });
  interactionActive.value = false;
  afterAction(fromOverflow);
}

function emitJump(direction: -1 | 1, fromOverflow = false) {
  const maxSeconds = Math.max(1, Math.floor(props.state.durationMs / 1_000));
  const seconds = Math.min(
    maxSeconds,
    Math.max(1, Math.floor(Number(jumpSeconds.value) || 1)),
  );
  jumpSeconds.value = seconds;
  const targetMs = Math.min(
    props.state.durationMs,
    Math.max(0, props.state.seekMs + direction * seconds * 1_000),
  );
  emit("action", { type: "seek", timeMs: targetMs });
  afterAction(fromOverflow);
}

function updateTimelineHover(event: PointerEvent) {
  const start = timelineStartTimeMs.value;
  const end = timelineEndTimeMs.value;
  const input = event.currentTarget as HTMLInputElement;
  const rect = input.getBoundingClientRect();
  if (start === undefined || end === undefined || end <= start || rect.width <= 0) return;
  const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
  hoverTimelineTimeMs.value = start + Math.round(((end - start) * ratio) / 1_000) * 1_000;
  hoverTimelineLeft.value = Math.min(93, Math.max(7, ratio * 100));
}

function clearTimelineHover() {
  hoverTimelineTimeMs.value = undefined;
}

function twoDigits(value: number) {
  return String(value).padStart(2, "0");
}

function formatTimelineBoundary(timeMs: number | undefined) {
  if (timeMs === undefined) return "--:--:--";
  const time = new Date(timeMs);
  return `${twoDigits(time.getMonth() + 1)}-${twoDigits(time.getDate())} ${formatTimelineTick(timeMs)}`;
}

function formatTimelineTick(timeMs: number) {
  const time = new Date(timeMs);
  return `${twoDigits(time.getHours())}:${twoDigits(time.getMinutes())}:${twoDigits(time.getSeconds())}`;
}

function formatTimelineTooltip(timeMs: number) {
  const time = new Date(timeMs);
  return `${time.getFullYear()}-${formatTimelineBoundary(timeMs)}`;
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
  overflow-x: hidden;
  pointer-events: auto;
  background: linear-gradient(0deg, rgba(0, 0, 0, 0.78), transparent);
}

.playback-timeline-row {
  position: absolute;
  right: 0;
  bottom: 52px;
  left: 0;
  padding: 4px 10px 0;
  pointer-events: auto;
  background: linear-gradient(0deg, rgba(0, 0, 0, 0.72), rgba(0, 0, 0, 0.2));
}

.control-bar.has-primary-timeline .primary-controls {
  background: rgba(0, 0, 0, 0.72);
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

.playback-clip-controls {
  display: inline-flex;
  flex: 0 0 auto;
  align-items: center;
  gap: 5px;
}

.playback-clip-controls select,
.playback-clip-controls button,
.clip-down-tip {
  height: 32px;
}

.clip-down-tip {
  display: inline-flex;
}

.playback-clip-controls button {
  min-width: 54px;
  border-color: var(--accent);
  color: var(--accent);
}

.clip-down-spinner {
  display: inline-block;
  width: 14px;
  height: 14px;
  border: 2px solid rgba(34, 211, 238, 0.35);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: clip-down-spin 700ms linear infinite;
}

@keyframes clip-down-spin {
  to {
    transform: rotate(360deg);
  }
}

.more-button {
  min-width: 52px;
}

.timeline {
  display: grid;
  flex: 1 0 460px;
  grid-template-columns: auto minmax(220px, 1fr) auto;
  grid-template-rows: 48px 30px;
  align-items: center;
  column-gap: 8px;
  row-gap: 2px;
  min-width: 460px;
  color: var(--muted);
}

.timeline > .timeline-boundary:first-child {
  grid-column: 1;
  grid-row: 1;
}

.timeline > .timeline-track {
  grid-column: 2;
  grid-row: 1;
}

.timeline > .timeline-track + .timeline-boundary {
  grid-column: 3;
  grid-row: 1;
}

.timeline-boundary {
  flex: 0 0 auto;
  color: rgba(226, 232, 240, 0.88);
  font-size: 11px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.timeline-track {
  position: relative;
  display: block;
  flex: 1 1 auto;
  min-width: 220px;
  height: 48px;
}

.timeline-rail {
  position: absolute;
  top: 23px;
  right: 0;
  left: 0;
  height: 4px;
  border-radius: 999px;
  background: rgba(148, 163, 184, 0.38);
  pointer-events: none;
}

.timeline-track input {
  position: absolute;
  top: 17px;
  left: 0;
  width: 100%;
  margin: 0;
}

.clip-selection {
  position: absolute;
  top: 23px;
  z-index: 1;
  height: 4px;
  border-radius: 999px;
  background: linear-gradient(90deg, #fbbf24, #fb7185);
  pointer-events: none;
}

.timeline-track input.clip-range {
  z-index: 2;
  appearance: none;
  background: transparent;
  pointer-events: none;
}

.timeline-track input.clip-range::-webkit-slider-runnable-track {
  height: 4px;
  background: transparent;
}

.timeline-track input.clip-range::-webkit-slider-thumb {
  width: 18px;
  height: 22px;
  margin-top: -9px;
  appearance: none;
  border: 0;
  border-radius: 0;
  background: #fbbf24;
  clip-path: polygon(0 0, 100% 0, 60% 38%, 60% 100%, 40% 100%, 40% 38%);
  filter: drop-shadow(0 0 2px rgba(0, 0, 0, 0.9));
  pointer-events: auto;
  cursor: ew-resize;
}

.timeline-track input.clip-range-b::-webkit-slider-thumb {
  background: #fb7185;
}

.timeline-track input.clip-range::-moz-range-track {
  height: 4px;
  background: transparent;
}

.timeline-track input.clip-range::-moz-range-thumb {
  width: 18px;
  height: 22px;
  border: 0;
  border-radius: 0;
  background: #fbbf24;
  clip-path: polygon(0 0, 100% 0, 60% 38%, 60% 100%, 40% 100%, 40% 38%);
  filter: drop-shadow(0 0 2px rgba(0, 0, 0, 0.9));
  pointer-events: auto;
  cursor: ew-resize;
}

.timeline-track input.clip-range-b::-moz-range-thumb {
  background: #fb7185;
}

.timeline-tooltip {
  position: absolute;
  top: 0;
  z-index: 1;
  padding: 1px 5px;
  transform: translateX(-50%);
  border-radius: 4px;
  background: rgba(3, 10, 24, 0.94);
  color: #f8fafc;
  font-size: 10px;
  font-variant-numeric: tabular-nums;
  line-height: 15px;
  white-space: nowrap;
}

.timeline-ticks {
  position: absolute;
  top: 34px;
  right: 0;
  left: 0;
  height: 12px;
  pointer-events: none;
}

.timeline-ticks > span {
  position: absolute;
  transform: translateX(-50%);
  color: rgba(148, 163, 184, 0.86);
  font-size: 9px;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}

.timeline-jump {
  display: inline-flex;
  grid-column: 2;
  grid-row: 2;
  align-items: center;
  justify-self: center;
  gap: 4px;
  color: rgba(226, 232, 240, 0.88);
  font-size: 11px;
  white-space: nowrap;
}

.timeline-jump button {
  height: 28px;
  padding: 0 7px;
}

.timeline-jump input {
  width: 54px;
  height: 28px;
  padding: 0 5px;
  border-radius: 5px;
  text-align: center;
}

.primary-timeline {
  width: 100%;
  min-width: 0;
  flex: none;
  grid-template-columns: auto minmax(80px, 1fr) auto max-content;
  grid-template-rows: 48px;
}

.primary-timeline-jump {
  grid-column: 4;
  grid-row: 1;
  justify-self: end;
  gap: clamp(2px, 0.5vw, 4px);
  font-size: clamp(9px, 1vw, 11px);
}

.primary-timeline-jump button {
  height: clamp(24px, 3vw, 28px);
  padding: 0 clamp(4px, 0.7vw, 7px);
}

.primary-timeline-jump input {
  width: clamp(42px, 5vw, 54px);
  height: clamp(24px, 3vw, 28px);
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

.control-bar.has-primary-timeline .overflow-menu {
  bottom: 104px;
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
