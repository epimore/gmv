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

    <div class="grid-body" :style="{ gridTemplateColumns: 'repeat(' + columnCount + ', minmax(0, 1fr))' }">
      <button
        v-for="(_, index) in modelGrid"
        :key="index"
        type="button"
        class="grid-cell"
        :class="{ selected: selectedIndex === index }"
        @click="selectedIndex = index"
        @dblclick="modelGrid = 1"
      >
        <GmvPlayerView
          v-if="cells[index]?.sources.length"
          v-bind="cells[index]"
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
        <span v-else class="empty-cell">空画面 {{ index + 1 }}</span>
      </button>
    </div>
  </section>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue';
import type { GmvAiBox, GmvDeviceStatus, GmvOsdItem, GmvPtzCommand, GmvSource, GmvViewCapabilities } from '../core/types';
import GmvPlayerView from './GmvPlayerView.vue';

export interface GmvGridCell {
  sources: GmvSource[];
  title?: string;
  deviceId?: string;
  channelId?: string;
  status?: GmvDeviceStatus;
  viewers?: number;
  osd?: GmvOsdItem[];
  aiBoxes?: GmvAiBox[];
  capabilities?: GmvViewCapabilities;
}

defineProps<{
  cells: GmvGridCell[];
}>();

const emit = defineEmits<{
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
}>();

const gridSizes = [1, 4, 9, 16];
const modelGrid = ref(4);
const selectedIndex = ref(0);
const columnCount = computed(() => Math.sqrt(modelGrid.value));
</script>
