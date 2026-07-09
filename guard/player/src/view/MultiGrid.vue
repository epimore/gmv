<template>
  <section class="multi-grid">
    <header class="grid-toolbar">
      <strong>多宫格</strong>
      <div>
        <button v-for="size in gridSizes" :key="size" type="button" :class="{ active: modelGrid === size }" @click="modelGrid = size">
          {{ size }}
        </button>
      </div>
    </header>

    <div class="grid-body" :style="gridBodyStyle">
      <div
        v-for="(_, index) in modelGrid"
        :key="index"
        role="button"
        tabindex="0"
        class="grid-cell"
        :class="{ selected: selectedIndex === index }"
        :draggable="!!cells[index]"
        @click="selectedIndex = index"
        @dblclick="modelGrid = 1"
        @dragstart="handleDragStart(index)"
        @dragover.prevent
        @drop="handleDrop(index)"
        @dragend="handleDragEnd"
        @keydown.enter="selectedIndex = index"
        @keydown.space.prevent="selectedIndex = index"
      >
        <button
          v-if="cells[index]"
          type="button"
          class="grid-cell-close"
          aria-label="关闭画面"
          @click.stop="emit('close', { index })"
        >
          ×
        </button>
        <GmvPlayerView
          v-if="cells[index]?.sources.length"
          v-bind="cells[index]"
          :controls-visible="controlsVisible"
          @snapshot="(payload) => emit('snapshot', { index, payload })"
          @record-start="(payload) => emit('recordStart', { index, payload })"
          @record-stop="(payload) => emit('recordStop', { index, payload })"
          @ptz="(payload) => emit('ptz', { index, payload })"
          @preset-call="(payload) => emit('presetCall', { index, payload })"
          @preset-set="(payload) => emit('presetSet', { index, payload })"
          @talk-start="() => emit('talkStart', { index })"
          @talk-stop="() => emit('talkStop', { index })"
          @playback-seek="(payload) => emit('playbackSeek', { index, payload })"
          @stream-switch="(payload) => emit('streamSwitch', { index, payload })"
        />
        <span v-else-if="cells[index]" class="empty-cell">
          <b>{{ cells[index]?.title || '等待播放' }}</b>
          <small>{{ cells[index]?.status === 'error' ? '播放失败' : '正在请求播放' }}</small>
        </span>
        <span v-else class="empty-cell">空画面 {{ index + 1 }}</span>
      </div>
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import type { GmvAiBox, GmvDeviceStatus, GmvOsdItem, GmvPtzCommand, GmvSource, GmvViewCapabilities } from '../core/types';
import GmvPlayerView from './GmvPlayerView.vue';

export interface GmvGridCell {
  sources: GmvSource[];
  title?: string;
  deviceId?: string;
  channelId?: string;
  status?: GmvDeviceStatus;
  viewers?: number;
  poster?: string;
  osd?: GmvOsdItem[];
  aiBoxes?: GmvAiBox[];
  capabilities?: GmvViewCapabilities;
}

const props = withDefaults(
  defineProps<{
    cells: GmvGridCell[];
    gridSize?: number;
    controlsVisible?: boolean;
  }>(),
  {
    controlsVisible: true,
  },
);

const emit = defineEmits<{
  'update:gridSize': [value: number];
  snapshot: [{ index: number; payload: { deviceId?: string; channelId?: string } }];
  recordStart: [{ index: number; payload: { deviceId?: string; channelId?: string } }];
  recordStop: [{ index: number; payload: { deviceId?: string; channelId?: string } }];
  ptz: [{ index: number; payload: GmvPtzCommand }];
  presetCall: [{ index: number; payload: { presetId: string } }];
  presetSet: [{ index: number; payload: { presetId: string } }];
  talkStart: [{ index: number }];
  talkStop: [{ index: number }];
  playbackSeek: [{ index: number; payload: { timeMs: number } }];
  streamSwitch: [{ index: number; payload: { source: GmvSource } }];
  close: [{ index: number }];
  reorder: [{ sourceIndex: number; targetIndex: number }];
}>();

const gridSizes = [1, 4, 9, 16];
const controlsVisible = computed(() => props.controlsVisible ?? true);
const localGrid = ref(props.gridSize ?? 4);
const selectedIndex = ref(0);
const draggingIndex = ref<number>();
const modelGrid = computed({
  get: () => props.gridSize ?? localGrid.value,
  set: (value: number) => {
    localGrid.value = value;
    emit('update:gridSize', value);
  },
});
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
  if (!props.cells[index]) return;
  draggingIndex.value = index;
}

function handleDrop(targetIndex: number) {
  const sourceIndex = draggingIndex.value;
  draggingIndex.value = undefined;
  if (sourceIndex === undefined || sourceIndex === targetIndex || !props.cells[sourceIndex] || !props.cells[targetIndex]) return;
  emit('reorder', { sourceIndex, targetIndex });
}

function handleDragEnd() {
  draggingIndex.value = undefined;
}
</script>
