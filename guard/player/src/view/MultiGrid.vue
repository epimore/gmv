<template>
  <section class="multi-grid">
    <header class="grid-toolbar">
      <div class="grid-toolbar-summary">
        <slot name="summary"><strong>多宫格</strong></slot>
      </div>
      <div class="grid-toolbar-actions">
        <strong v-if="$slots.summary" class="grid-layout-title">多宫格</strong>
        <div class="grid-size-options">
          <button v-for="size in gridSizes" :key="size" type="button" :class="{ active: modelGrid === size }" @click="modelGrid = size">
            {{ size }}
          </button>
        </div>
        <slot name="actions" />
      </div>
    </header>

    <div class="grid-body" :style="gridBodyStyle">
      <div
        v-for="(_, index) in cells"
        :key="cellIdentity(index)"
        v-show="isVisibleCell(index)"
        role="button"
        tabindex="0"
        class="grid-cell"
        :class="{ selected: selectedIndex === visibleIndex(index) }"
        :draggable="isVisibleCell(index)"
        @click="selectedIndex = visibleIndex(index)"
        @dblclick="modelGrid = 1"
        @dragstart="handleDragStart(index)"
        @dragover.prevent
        @drop="handleDrop(index)"
        @dragend="handleDragEnd"
        @keydown.enter="selectedIndex = visibleIndex(index)"
        @keydown.space.prevent="selectedIndex = visibleIndex(index)"
      >
        <button
          v-if="cells[index]"
          type="button"
          class="grid-cell-close"
          aria-label="关闭画面"
          @click.stop="emit('close', { index: visibleIndex(index) })"
        >
          ×
        </button>
        <GmvPlayerView
          v-if="cells[index]?.sources.length"
          :ref="playerRefFor(cellIdentity(index))"
          v-bind="playerProps(cells[index])"
          @snapshot="(payload) => emit('snapshot', { index: visibleIndex(index), payload })"
          @snapshot-error="(payload) => emit('snapshotError', { index: visibleIndex(index), payload })"
          @record-start="(payload) => emit('recordStart', { index: visibleIndex(index), payload })"
          @record-stop="(payload) => emit('recordStop', { index: visibleIndex(index), payload })"
          @ptz="(payload) => emit('ptz', { index: visibleIndex(index), payload })"
          @preset-call="(payload) => emit('presetCall', { index: visibleIndex(index), payload })"
          @preset-set="(payload) => emit('presetSet', { index: visibleIndex(index), payload })"
          @playback-seek="(payload) => emit('playbackSeek', { index: visibleIndex(index), payload })"
          @playback-rate-change="(payload) => emit('playbackRateChange', { index: visibleIndex(index), payload })"
          @playback-state-change="(payload) => emit('playbackStateChange', { index: visibleIndex(index), payload })"
          @playback-progress="(payload) => emit('playbackProgress', { index: visibleIndex(index), payload })"
          @cloud-record-create="(payload) => emit('cloudRecordCreate', { index: visibleIndex(index), payload })"
          @stream-switch="(payload) => emit('streamSwitch', { index: visibleIndex(index), payload })"
          @output-type-change="(outputType) => emit('outputTypeChange', { index: visibleIndex(index), outputType })"
          @playing="(payload) => emit('playing', { index: visibleIndex(index), payload })"
          @playback-error="(payload) => emit('playbackError', { index: visibleIndex(index), payload })"
          @playback-switch-cancel="() => emit('playbackSwitchCancel', { index: visibleIndex(index) })"
        />
        <span v-else-if="cells[index]" class="empty-cell">
          <b>{{ cells[index]?.title || '等待播放' }}</b>
          <small>{{ cells[index]?.status === 'error' ? '播放失败' : cells[index]?.startupText || '正在请求播放' }}</small>
        </span>
      </div>
      <div
        v-for="slot in emptySlotCount"
        :key="`empty-${pageStart}-${slot}`"
        role="button"
        tabindex="0"
        class="grid-cell"
        :class="{ selected: selectedIndex === visibleCellCount + slot - 1 }"
        @click="selectedIndex = visibleCellCount + slot - 1"
        @dblclick="modelGrid = 1"
        @keydown.enter="selectedIndex = visibleCellCount + slot - 1"
        @keydown.space.prevent="selectedIndex = visibleCellCount + slot - 1"
      >
        <span class="empty-cell">空画面 {{ visibleCellCount + slot }}</span>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, ref, watch, type ComponentPublicInstance } from 'vue';
import type { GmvAiBox, GmvCloudRecordRange, GmvDeviceStatus, GmvMediaMode, GmvOsdItem, GmvPlayerControlsConfig, GmvPlayerOutputOption, GmvPtzCommand, GmvSource, GmvViewCapabilities } from '../core/types';
import GmvPlayerView from './GmvPlayerView.vue';

export interface GmvGridCell {
  cellId?: string;
  sources: GmvSource[];
  title?: string;
  deviceId?: string;
  channelId?: string;
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
  cloudRecordLockedRange?: GmvCloudRecordRange;
}

const props = defineProps<{
  cells: GmvGridCell[];
  gridSize?: number;
  visibleStart?: number;
}>();

const emit = defineEmits<{
  'update:gridSize': [value: number];
  snapshot: [{ index: number; payload: { deviceId?: string; channelId?: string; fileName: string } }];
  snapshotError: [{ index: number; payload: { message: string } }];
  recordStart: [{ index: number; payload: { deviceId?: string; channelId?: string } }];
  recordStop: [{ index: number; payload: { deviceId?: string; channelId?: string } }];
  ptz: [{ index: number; payload: GmvPtzCommand }];
  presetCall: [{ index: number; payload: { presetId: string } }];
  presetSet: [{ index: number; payload: { presetId: string } }];
  playbackSeek: [{ index: number; payload: { timeMs: number } }];
  playbackRateChange: [{ index: number; payload: { rate: number } }];
  playbackStateChange: [{ index: number; payload: { paused: boolean } }];
  playbackProgress: [{ index: number; payload: { mediaTimeMs: number } }];
  cloudRecordCreate: [{ index: number; payload: { startTimeMs: number; endTimeMs: number } }];
  streamSwitch: [{ index: number; payload: { source: GmvSource } }];
  outputTypeChange: [{ index: number; outputType: string }];
  playing: [{ index: number; payload: { source?: GmvSource } }];
  playbackError: [{ index: number; payload: { message: string; source?: GmvSource } }];
  playbackSwitchCancel: [{ index: number }];
  close: [{ index: number }];
  reorder: [{ sourceIndex: number; targetIndex: number }];
}>();

const gridSizes = [1, 4, 9, 16];
const localGrid = ref(props.gridSize ?? 4);
const selectedIndex = ref(0);
const draggingIndex = ref<number>();
type PlayerInstance = ComponentPublicInstance & {
  confirmPlaybackRate: (rate: number) => void;
  confirmPlaybackState: (paused: boolean) => void;
  confirmPlaybackProgress: (timeMs: number) => void;
};
const playerRefs = new Map<string, PlayerInstance>();
const playerRefBindings = new Map<string, (instance: unknown) => void>();
const modelGrid = computed({
  get: () => props.gridSize ?? localGrid.value,
  set: (value: number) => {
    localGrid.value = value;
    emit('update:gridSize', value);
  },
});
const pageStart = computed(() => Math.max(0, props.visibleStart ?? 0));
const visibleCellCount = computed(() => Math.min(modelGrid.value, Math.max(0, props.cells.length - pageStart.value)));
const emptySlotCount = computed(() => modelGrid.value - visibleCellCount.value);
const columnCount = computed(() => Math.sqrt(modelGrid.value));
const gridBodyStyle = computed(() => ({
  display: 'grid',
  gridTemplateColumns: 'repeat(' + columnCount.value + ', minmax(0, 1fr))',
  gridTemplateRows: 'repeat(' + columnCount.value + ', minmax(0, 1fr))',
}));

watch(() => props.gridSize, (value) => {
  if (value) localGrid.value = value;
});

function handleDragStart(index: number) {
  if (!isVisibleCell(index)) return;
  draggingIndex.value = index;
}

function handleDrop(targetIndex: number) {
  const sourceIndex = draggingIndex.value;
  draggingIndex.value = undefined;
  if (sourceIndex === undefined || sourceIndex === targetIndex || !isVisibleCell(sourceIndex) || !isVisibleCell(targetIndex)) return;
  emit('reorder', { sourceIndex: visibleIndex(sourceIndex), targetIndex: visibleIndex(targetIndex) });
}

function handleDragEnd() {
  draggingIndex.value = undefined;
}

function cellIdentity(index: number) {
  return props.cells[index]?.cellId || `grid-slot-${index}`;
}

function visibleIndex(index: number) {
  return index - pageStart.value;
}

function isVisibleCell(index: number) {
  const pageIndex = visibleIndex(index);
  return pageIndex >= 0 && pageIndex < modelGrid.value;
}

function playerProps(cell: GmvGridCell | undefined): Omit<GmvGridCell, 'cellId'> {
  if (!cell) return { sources: [] };
  const { cellId: _, ...player } = cell;
  return player;
}

function playerRefFor(cellId: string) {
  const existing = playerRefBindings.get(cellId);
  if (existing) return existing;
  const binding = (instance: unknown) => {
    if (instance) {
      playerRefs.set(cellId, instance as PlayerInstance);
    } else {
      playerRefs.delete(cellId);
      playerRefBindings.delete(cellId);
    }
  };
  playerRefBindings.set(cellId, binding);
  return binding;
}

function confirmPlaybackRate(index: number, rate: number) {
  playerRefs.get(cellIdentity(pageStart.value + index))?.confirmPlaybackRate(rate);
}

function confirmPlaybackState(index: number, paused: boolean) {
  playerRefs.get(cellIdentity(pageStart.value + index))?.confirmPlaybackState(paused);
}

function confirmPlaybackProgress(index: number, timeMs: number) {
  playerRefs.get(cellIdentity(pageStart.value + index))?.confirmPlaybackProgress(timeMs);
}

defineExpose({ confirmPlaybackRate, confirmPlaybackState, confirmPlaybackProgress });

</script>
