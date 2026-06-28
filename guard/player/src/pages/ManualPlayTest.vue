<template>
  <main class="manual-page">
    <section class="manual-toolbar">
      <div>
        <h1>GmvPlayer 人工播放测试</h1>
        <p>输入 GMV 播放地址后加入当前选中画面，用于 FLV / FMP4 / HLS 人工验收。</p>
      </div>
      <form class="source-form" @submit.prevent="addSource">
        <input v-model="form.url" placeholder="http://127.0.0.1:18570/s1/play/stream.flv" />
        <select v-model="form.protocol">
          <option value="flv">FLV</option>
          <option value="fmp4">FMP4</option>
          <option value="hls">HLS</option>
        </select>
        <select v-model="form.codec">
          <option value="h264">H.264</option>
          <option value="h265">H.265</option>
        </select>
        <select v-model="form.hasAudio">
          <option :value="false">无音频/G711</option>
          <option :value="true">AAC音频</option>
        </select>
        <input v-model="form.mimeCodec" placeholder="video/mp4; codecs=&quot;avc1.42E01E, mp4a.40.2&quot;" />
        <button type="submit">加入画面</button>
      </form>
    </section>

    <GmvMultiGrid
      :cells="cells"
      @ptz="logAction('ptz', $event)"
      @snapshot="logAction('snapshot', $event)"
      @record-start="logAction('recordStart', $event)"
      @record-stop="logAction('recordStop', $event)"
      @preset-call="logAction('presetCall', $event)"
      @preset-set="logAction('presetSet', $event)"
      @talk-start="logAction('talkStart', $event)"
      @talk-stop="logAction('talkStop', $event)"
      @playback-seek="logAction('playbackSeek', $event)"
      @stream-switch="logAction('streamSwitch', $event)"
    />

    <section class="manual-log">
      <header>
        <strong>事件日志</strong>
        <button type="button" @click="logs = []">清空</button>
      </header>
      <pre>{{ logs.join('\n') }}</pre>
    </section>
  </main>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue';
import type { GmvCodec, GmvProtocol } from '../core/types';
import GmvMultiGrid, { type GmvGridCell } from '../view/MultiGrid.vue';

const cells = ref<GmvGridCell[]>(
  Array.from({ length: 16 }, (_, index) => ({
    sources: [],
    title: `画面 ${index + 1}`,
    deviceId: 'manual-device',
    channelId: `manual-channel-${index + 1}`,
    status: 'online',
    viewers: index === 0 ? 1 : undefined,
    osd: [
      { id: 'name', text: `CH-${index + 1}`, x: 3, y: 5 },
      { id: 'time', text: new Date().toLocaleString(), x: 3, y: 12 },
    ],
    aiBoxes: index === 0 ? [{ id: 'ai-1', label: '目标', confidence: 0.92, x: 38, y: 28, width: 18, height: 26 }] : [],
    capabilities: {
      ptz: true,
      presets: true,
      snapshot: true,
      record: true,
      playback: true,
      talk: true,
      streamSwitch: true,
      aiOverlay: true,
    },
  })),
);

const form = reactive({
  url: '',
  protocol: 'flv' as GmvProtocol,
  codec: 'h264' as GmvCodec,
  mimeCodec: 'video/mp4; codecs="avc1.42E01E, mp4a.40.2"',
  hasAudio: false,
});
const logs = ref<string[]>([]);
const nextCell = ref(0);

function addSource() {
  if (!form.url.trim()) return;

  const index = nextCell.value % cells.value.length;
  cells.value[index] = {
    ...cells.value[index],
    sources: [
      {
        protocol: form.protocol,
        codec: form.codec,
        url: form.url.trim(),
        mimeCodec: form.protocol === 'fmp4' ? form.mimeCodec.trim() : undefined,
        hasAudio: form.hasAudio,
        label: form.protocol.toUpperCase() + " " + form.codec.toUpperCase() + " " + (form.hasAudio ? "audio" : "video-only"),
        priority: 1,
      },
    ],
    title: `测试画面 ${index + 1}`,
    status: 'online',
  };
  nextCell.value = index + 1;
  logAction('addSource', { index, protocol: form.protocol, url: form.url });
}

function logAction(name: string, payload: unknown) {
  logs.value.unshift(`[${new Date().toLocaleTimeString()}] ${name} ${JSON.stringify(payload)}`);
  logs.value = logs.value.slice(0, 80);
}
</script>
