<template>
  <div v-if="!selectedDevice" class="page-grid" v-loading="loading">
    <GlassPanel class="span-12" title="监控信息" subtitle="按设备查看在线状态和注册时间">
      <div class="toolbar">
        <el-input v-model="keyword" style="width: 260px" clearable placeholder="搜索设备 ID / 名称" />
        <el-button :loading="loading" @click="loadDevices">刷新</el-button>
      </div>
      <el-table :data="filteredDevices" height="620" empty-text="暂无监控设备">
        <el-table-column type="index" label="序号" width="64" />
        <el-table-column prop="device_id" label="设备 ID" min-width="190" show-overflow-tooltip />
        <el-table-column label="设备名称" min-width="160" show-overflow-tooltip>
          <template #default="{ row }">{{ displayDeviceName(row) }}</template>
        </el-table-column>
        <el-table-column prop="domain_id" label="SIP服务器ID" min-width="190" show-overflow-tooltip />
        <el-table-column prop="domain" label="SIP域" min-width="120" show-overflow-tooltip />
        <el-table-column label="状态" width="100">
          <template #default="{ row }">
            <StatusPill :label="row.status === 1 ? '在线' : '离线'" :tone="row.status === 1 ? 'ONLINE' : 'OFFLINE'" />
          </template>
        </el-table-column>
        <el-table-column label="注册时间" min-width="170" show-overflow-tooltip>
          <template #default="{ row }">{{ row.create_time || '-' }}</template>
        </el-table-column>
        <el-table-column label="操作" width="110" fixed="right">
          <template #default="{ row }">
            <el-button type="primary" link @click="openChannels(row)">相机</el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>
  </div>

  <div v-else class="page-grid">
    <GlassPanel class="span-12" title="通道监控" :subtitle="selectedDevice.device_id">
      <div class="monitor-head">
        <div class="device-summary">
          <StatusPill :label="selectedDevice.status === 1 ? '在线' : '离线'" :tone="selectedDevice.status === 1 ? 'ONLINE' : 'OFFLINE'" />
          <strong>{{ displayDeviceName(selectedDevice) }}</strong>
          <span>Session {{ selectedDevice.session_node_id || '-' }}</span>
        </div>
        <div class="monitor-actions">
          <el-button :loading="channelLoading" @click="reloadChannels">刷新通道</el-button>
          <el-button type="primary" @click="backToDevices">返回</el-button>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel v-if="showImages" class="span-12" title="抓拍图集" :subtitle="selectedChannelTitle">
      <div class="toolbar">
        <el-button @click="showImages = false">返回通道</el-button>
        <el-button :loading="imageLoading" @click="selectedChannel && loadImages(selectedChannel)">刷新图集</el-button>
      </div>
      <div v-if="images.length" class="image-grid">
        <a v-for="image in images" :key="image.image_id" class="image-card" :href="image.image_url" target="_blank" rel="noreferrer">
          <div class="image-preview">
            <img v-if="image.image_url" :src="image.image_url" :alt="image.image_id" />
            <span v-else>暂无图片</span>
          </div>
          <div class="image-meta">
            <b>{{ image.image_id }}</b>
            <span>{{ formatTime(image.created_at_ms) }}</span>
          </div>
        </a>
      </div>
      <el-empty v-else description="暂无抓拍图片" />
    </GlassPanel>

    <template v-else>
      <GlassPanel class="span-12" title="相机列表" subtitle="与 gb28181_app 监控信息一致，从设备下钻到通道操作">
        <div v-loading="channelLoading" class="channel-grid">
          <article v-for="channel in sortedChannels" :key="channel.channel_id" class="channel-card">
            <header class="channel-card-head">
              <div>
                <h2>{{ displayChannelName(channel) }}</h2>
                <p>{{ channel.channel_id }}</p>
              </div>
              <StatusPill :label="channelStatusText(channel)" :tone="channelOnline(channel) ? 'ONLINE' : 'OFFLINE'" />
            </header>
            <button class="channel-cover" type="button" :disabled="!channel.pic_url" @click="previewCover(channel)">
              <img v-if="channel.pic_url" :src="channel.pic_url" :alt="displayChannelName(channel)" />
              <span v-else>暂无封面</span>
            </button>
            <div class="channel-tags">
              <span>{{ channel.ptz_type || 'PTZ -' }}</span>
              <span>{{ confText(channel.playback_enable, 2, '回放') }}</span>
              <span>{{ confText(channel.snapshot, 2, '抓拍') }}</span>
              <span>{{ confText(channel.biz_enable, 1, '业务') }}</span>
            </div>
            <footer class="channel-actions">
              <el-button :disabled="!canPlayLive(channel)" @click="startPlay('preview', channel)">实时直播</el-button>
              <el-button :disabled="!canPlayback(channel)" @click="startPlay('playback', channel)">历史回放</el-button>
              <el-button :disabled="!canSnapshot(channel)" :loading="snapshotLoading[channel.channel_id]" @click="snapshot(channel)">抓拍</el-button>
              <el-button :disabled="!canViewImages(channel)" @click="openImages(channel)">抓拍图集</el-button>
              <el-button :disabled="!canOperate" @click="openConfig(channel)">配置</el-button>
            </footer>
          </article>
        </div>
        <el-empty v-if="!channelLoading && !sortedChannels.length" description="暂无通道" />
      </GlassPanel>

      <GlassPanel class="span-12" title="播放窗口" :subtitle="playerSubtitle">
        <div v-if="playerSources.length" class="monitor-player">
          <GmvPlayerView :sources="playerSources" :device-id="selectedChannel?.device_id"
            :channel-id="selectedChannel?.channel_id" :title="selectedChannelTitle" :status="playerStatus" :viewers="1"
            :osd="playerOsd" :capabilities="playerCapabilities" @snapshot="selectedChannel && snapshot(selectedChannel)"
            @ptz="handlePlayerPtz" />
        </div>
        <el-empty v-else description="选择在线通道后播放" />
      </GlassPanel>
    </template>

    <el-dialog v-model="coverDialog" title="封面快照" width="720px">
      <img v-if="coverUrl" class="cover-large" :src="coverUrl" alt="封面快照" />
      <el-empty v-else description="暂无封面" />
    </el-dialog>

    <el-drawer v-model="configDrawer" title="相机业务配置" size="420px" destroy-on-close>
      <el-form :model="configForm" label-width="110px" class="config-form">
        <el-form-item label="设备ID"><el-input v-model="configForm.device_id" disabled /></el-form-item>
        <el-form-item label="通道ID"><el-input v-model="configForm.channel_id" disabled /></el-form-item>
        <el-form-item label="名称"><el-input v-model="configForm.name" disabled /></el-form-item>
        <el-form-item label="别名"><el-input v-model="configForm.alias_name" maxlength="16" clearable /></el-form-item>
        <el-form-item label="排序"><el-input-number v-model="configForm.sort_no" :min="0" :max="999999" /></el-form-item>
        <el-form-item label="云台控制"><el-select v-model="configForm.ptz_enable"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="语音对讲"><el-select v-model="configForm.talk_enable"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="音频"><el-select v-model="configForm.audio_enable"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="抓拍"><el-select v-model="configForm.snapshot"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="录像"><el-select v-model="configForm.record_enable"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="回放"><el-select v-model="configForm.playback_enable"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="告警"><el-select v-model="configForm.alarm_enable"><el-option v-for="option in confOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="业务启用"><el-select v-model="configForm.biz_enable"><el-option v-for="option in bizOptions" :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
      </el-form>
      <template #footer>
        <div class="drawer-footer">
          <el-button @click="configDrawer = false">取消</el-button>
          <el-button type="primary" :loading="configSaving" :disabled="!canOperate" @click="saveConfig">保存</el-button>
        </div>
      </template>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import {
  listGbChannelImages,
  listGbChannels,
  listGbDevices,
  sendGbPtz,
  startGbPlayback,
  startGbPreview,
  takeGbSnapshot,
  updateGbChannel,
  type GbChannelImageInfo,
  type GbChannelInfo,
  type GbChannelPayload,
  type GbDeviceInfo,
  type StreamSummary,
} from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { GmvPlayerView, type GmvPtzCommand, type GmvSource } from 'gmv-player';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const loading = ref(false);
const channelLoading = ref(false);
const imageLoading = ref(false);
const configSaving = ref(false);
const keyword = ref('');
const devices = ref<GbDeviceInfo[]>([]);
const channels = ref<GbChannelInfo[]>([]);
const images = ref<GbChannelImageInfo[]>([]);
const selectedDevice = ref<GbDeviceInfo>();
const selectedChannel = ref<GbChannelInfo>();
const lastStream = ref<StreamSummary>();
const lastAction = ref('');
const showImages = ref(false);
const coverDialog = ref(false);
const coverUrl = ref('');
const configDrawer = ref(false);
const snapshotLoading = reactive<Record<string, boolean>>({});
const configForm = reactive<GbChannelPayload & { device_id?: string }>({ channel_id: '', device_id: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');

const confOptions = [
  { label: '启用', value: 1 },
  { label: '禁用', value: 0 },
  { label: '设备不支持', value: 2 },
];
const bizOptions = [
  { label: '启用', value: 1 },
  { label: '禁用', value: 0 },
];
const playerCapabilities = {
  ptz: true,
  presets: false,
  snapshot: true,
  record: false,
  playback: true,
  talk: false,
  streamSwitch: false,
  aiOverlay: false,
};
const filteredDevices = computed(() => devices.value.filter((item) => {
  const key = keyword.value.trim();
  return !key || item.device_id.includes(key) || (item.alias || '').includes(key) || item.domain_id.includes(key);
}));
const sortedChannels = computed(() => [...channels.value].sort((left, right) => {
  const sortNo = Number(left.sort_no || 0) - Number(right.sort_no || 0);
  return sortNo || displayChannelName(left).localeCompare(displayChannelName(right), 'zh-Hans-CN');
}));
const selectedChannelTitle = computed(() => selectedChannel.value ? displayChannelName(selectedChannel.value) : '未选择通道');
const playerSubtitle = computed(() => lastStream.value?.endpoint || '选择在线通道后播放');
const playerStatus = computed(() => lastStream.value?.state === 'running' ? 'playing' : selectedChannel.value && channelOnline(selectedChannel.value) ? 'online' : 'idle');
const playerOsd = computed(() => [
  { id: 'channel', text: selectedChannelTitle.value, x: 3, y: 5 },
  { id: 'mode', text: lastAction.value || 'monitor', x: 3, y: 12 },
]);
const playerSources = computed<GmvSource[]>(() => {
  const endpoint = lastStream.value?.endpoint;
  if (!endpoint) return [];
  const protocol = streamProtocol(endpoint);
  return [{
    protocol,
    codec: 'h265',
    url: endpoint,
    mimeCodec: protocol === 'fmp4' ? 'video/mp4; codecs="hvc1.1.6.L123.B0, mp4a.40.2"' : undefined,
    hasAudio: false,
    label: '默认静音',
    priority: 1,
  }];
});

function displayDeviceName(device: GbDeviceInfo) { return device.alias || device.device_id; }
function displayChannelName(channel: GbChannelInfo) { return channel.alias_name || channel.name || channel.channel_id; }
function confValue(value: unknown, defaultValue = 2) { return value === undefined || value === null ? defaultValue : Number(value); }
function confEnabled(value: unknown) { return confValue(value) === 1; }
function bizEnabled(channel: GbChannelInfo) { return confValue(channel.biz_enable, 1) === 1; }
function channelOnline(channel: GbChannelInfo) { return ['ON', 'ONLINE'].includes((channel.status || '').toUpperCase()); }
function channelStatusText(channel: GbChannelInfo) { return channelOnline(channel) ? '在线' : '离线'; }
function confText(value: unknown, defaultValue: number, label: string) {
  const v = confValue(value, defaultValue);
  if (v === 1) return label + '启用';
  if (v === 0) return label + '禁用';
  return label + '不支持';
}
function canPlayLive(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel); }
function canPlayback(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.playback_enable); }
function canSnapshot(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.snapshot); }
function canViewImages(channel: GbChannelInfo) { return bizEnabled(channel); }
function streamProtocol(endpoint: string): GmvSource['protocol'] {
  const path = endpoint.split('?')[0].toLowerCase();
  if (path.endsWith('.fmp4')) return 'fmp4';
  if (path.endsWith('.m3u8')) return 'hls';
  return 'flv';
}
function formatTime(value: number) {
  if (!value) return '-';
  return new Date(value).toLocaleString();
}

async function loadDevices() {
  loading.value = true;
  try {
    devices.value = await listGbDevices();
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '设备列表加载失败');
  } finally {
    loading.value = false;
  }
}
async function openChannels(device: GbDeviceInfo) {
  selectedDevice.value = device;
  selectedChannel.value = undefined;
  lastStream.value = undefined;
  lastAction.value = '';
  showImages.value = false;
  await reloadChannels();
}
async function reloadChannels() {
  if (!selectedDevice.value) return;
  channelLoading.value = true;
  try {
    channels.value = await listGbChannels(selectedDevice.value.device_id);
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '通道列表加载失败');
  } finally {
    channelLoading.value = false;
  }
}
function backToDevices() {
  selectedDevice.value = undefined;
  selectedChannel.value = undefined;
  channels.value = [];
  images.value = [];
  lastStream.value = undefined;
  showImages.value = false;
}
async function startPlay(kind: 'preview' | 'playback', channel: GbChannelInfo) {
  selectedChannel.value = channel;
  showImages.value = false;
  try {
    lastStream.value = kind === 'preview'
      ? await startGbPreview(channel.device_id, channel.channel_id, { request_id: 'ui-monitor-preview-' + Date.now(), output_type: 'flv' })
      : await startGbPlayback(channel.device_id, channel.channel_id, { request_id: 'ui-monitor-playback-' + Date.now(), output_type: 'flv' });
    lastAction.value = kind === 'preview' ? '实时直播' : '历史回放';
    ElMessage.success(lastAction.value + '已提交');
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '播放请求失败');
  }
}
async function snapshot(channel: GbChannelInfo) {
  if (snapshotLoading[channel.channel_id]) return;
  snapshotLoading[channel.channel_id] = true;
  try {
    await takeGbSnapshot(channel.device_id, channel.channel_id);
    selectedChannel.value = channel;
    await loadImages(channel);
    ElMessage.success('抓拍已提交');
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '抓拍失败');
  } finally {
    snapshotLoading[channel.channel_id] = false;
  }
}
async function loadImages(channel: GbChannelInfo) {
  imageLoading.value = true;
  try {
    images.value = await listGbChannelImages(channel.device_id, channel.channel_id);
  } catch (error) {
    images.value = [];
    ElMessage.error(error instanceof Error ? error.message : '抓拍图集加载失败');
  } finally {
    imageLoading.value = false;
  }
}
async function openImages(channel: GbChannelInfo) {
  selectedChannel.value = channel;
  showImages.value = true;
  await loadImages(channel);
}
function previewCover(channel: GbChannelInfo) {
  coverUrl.value = channel.pic_url || '';
  coverDialog.value = true;
}
function openConfig(channel: GbChannelInfo) {
  selectedChannel.value = channel;
  Object.assign(configForm, {
    device_id: channel.device_id,
    channel_id: channel.channel_id,
    name: channel.name,
    alias_name: channel.alias_name || '',
    ptz_enable: confValue(channel.ptz_enable),
    talk_enable: confValue(channel.talk_enable),
    audio_enable: confValue(channel.audio_enable),
    snapshot: confValue(channel.snapshot),
    record_enable: confValue(channel.record_enable),
    playback_enable: confValue(channel.playback_enable),
    alarm_enable: confValue(channel.alarm_enable),
    biz_enable: confValue(channel.biz_enable, 1),
    sort_no: Number(channel.sort_no || 0),
  });
  configDrawer.value = true;
}
async function saveConfig() {
  if (!selectedChannel.value) return;
  configSaving.value = true;
  try {
    const payload = { ...configForm };
    delete payload.device_id;
    await updateGbChannel(selectedChannel.value.device_id, selectedChannel.value.channel_id, payload);
    configDrawer.value = false;
    await reloadChannels();
    ElMessage.success('业务配置已保存');
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '业务配置保存失败');
  } finally {
    configSaving.value = false;
  }
}
async function handlePlayerPtz(command: GmvPtzCommand) {
  if (!selectedChannel.value || command.action === 'stop') return;
  try {
    await sendGbPtz(selectedChannel.value.device_id, selectedChannel.value.channel_id);
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '云台控制失败');
  }
}

onMounted(loadDevices);
</script>

<style scoped>
.monitor-head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 14px;
  align-items: center;
}

.device-summary,
.monitor-actions {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}

.device-summary strong {
  font-size: 17px;
}

.device-summary span {
  color: var(--muted);
  font-size: 13px;
}

.channel-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(300px, 1fr));
  gap: 14px;
  min-height: 160px;
}

.channel-card {
  min-width: 0;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
}

.channel-card-head {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 10px;
  align-items: start;
  padding: 12px;
}

.channel-card-head h2 {
  margin: 0;
  overflow: hidden;
  color: var(--text);
  font-size: 16px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.channel-card-head p {
  margin: 5px 0 0;
  overflow: hidden;
  color: var(--muted);
  font-size: 12px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.channel-cover {
  display: grid;
  place-items: center;
  width: 100%;
  aspect-ratio: 4 / 3;
  border: 0;
  border-top: 1px solid rgba(100, 203, 255, .12);
  border-bottom: 1px solid rgba(100, 203, 255, .12);
  background: rgba(2, 5, 10, .88);
  color: var(--muted);
  cursor: pointer;
}

.channel-cover:disabled {
  cursor: default;
}

.channel-cover img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.channel-tags {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
  padding: 10px 12px 0;
}

.channel-tags span {
  padding: 4px 8px;
  border: 1px solid rgba(100, 203, 255, .14);
  border-radius: 999px;
  color: var(--cyan);
  background: rgba(255, 255, 255, .03);
  font-size: 12px;
}

.channel-actions {
  display: grid;
  grid-template-columns: repeat(5, minmax(0, 1fr));
  gap: 8px;
  padding: 12px;
}

.channel-actions .el-button {
  width: 100%;
  margin-left: 0;
}

.monitor-player {
  height: 420px;
  min-height: 420px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
}

.monitor-player :deep(.gmv-player) {
  position: relative;
  width: 100%;
  height: 100%;
  min-height: 420px;
  overflow: hidden;
  background: #02050a;
  color: var(--text);
}

.monitor-player :deep(.gmv-video) {
  width: 100%;
  height: 100%;
  display: block;
  object-fit: contain;
  background: #02050a;
}

.image-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(220px, 1fr));
  gap: 12px;
}

.image-card {
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  color: inherit;
  background: rgba(3, 10, 24, .36);
  text-decoration: none;
}

.image-preview {
  display: grid;
  place-items: center;
  aspect-ratio: 4 / 3;
  background: rgba(2, 5, 10, .88);
  color: var(--muted);
}

.image-preview img {
  width: 100%;
  height: 100%;
  object-fit: cover;
}

.image-meta {
  display: grid;
  gap: 4px;
  padding: 10px;
}

.image-meta b {
  overflow: hidden;
  color: var(--text);
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.image-meta span {
  color: var(--muted);
  font-size: 12px;
}

.cover-large {
  display: block;
  width: 100%;
  max-height: 70vh;
  object-fit: contain;
  background: #02050a;
}

.config-form :deep(.el-select),
.config-form :deep(.el-input-number) {
  width: 100%;
}

.drawer-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
}

@media (max-width: 900px) {
  .monitor-head {
    grid-template-columns: 1fr;
  }

  .monitor-actions {
    justify-content: flex-start;
  }

  .channel-grid,
  .image-grid {
    grid-template-columns: 1fr;
  }

  .channel-actions {
    grid-template-columns: 1fr 1fr;
  }
}
</style>
