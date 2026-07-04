<template>
  <div v-if="!selectedDevice && monitorMode === 'devices'" class="page-grid" v-loading="loading">
    <GlassPanel class="span-12" title="监控信息" subtitle="按设备查看在线状态和注册时间">
      <div class="toolbar">
        <el-select v-model="selectedListNodeId" filterable placeholder="选择 Session 节点" style="width: 420px"
          :loading="listNodeLoading" @change="handleListNodeChange">
          <el-option v-for="option in sessionNodeOptions" :key="option.node.node_id" :label="listNodeLabel(option)"
            :value="option.node.node_id" :disabled="option.disabled">
            <div class="node-option" :class="{ offline: option.disabled }">
              <span>{{ option.kindLabel }} · {{ option.node.node_id }}</span>
              <span class="node-status">{{ option.statusLabel }}</span>
            </div>
          </el-option>
        </el-select>
        <el-input v-model="deviceName" style="width: 220px" clearable placeholder="设备名称" @clear="queryDevices" />
        <el-button type="primary" :loading="loading" @click="queryDevices">查询</el-button>
        <el-button :loading="loading" @click="resetDevices">重置</el-button>
        <el-button :loading="loading" @click="loadDevices">刷新</el-button>
        <el-button type="primary" plain @click="openMultiView">通道树多画面</el-button>
      </div>
      <el-table :data="devices" height="620" empty-text="暂无监控设备">
        <el-table-column type="index" :index="tableIndex" label="序号" width="64" />
        <el-table-column prop="device_id" label="设备 ID" min-width="200" show-overflow-tooltip />
        <el-table-column label="设备名称" min-width="160" show-overflow-tooltip>
          <template #default="{ row }">{{ displayDeviceName(row) }}</template>
        </el-table-column>
        <el-table-column label="状态" width="100">
          <template #default="{ row }">
            <StatusPill :label="row.monitor_status === 1 ? '在线' : '离线'"
              :tone="row.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
          </template>
        </el-table-column>
        <el-table-column label="国标版本" width="100">
          <template #default="{ row }">{{ emptyText(row.gb_version) }}</template>
        </el-table-column>
        <el-table-column label="注册时间" min-width="170" show-overflow-tooltip>
          <template #default="{ row }">{{ row.register_time || '-' }}</template>
        </el-table-column>
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button link @click="openDeviceDetail(row)">查看</el-button>
            <el-button type="primary" link @click="openChannels(row)">相机</el-button>
          </template>
        </el-table-column>
      </el-table>
      <div class="pagination-bar">
        <el-pagination v-model:current-page="page" v-model:page-size="pageSize" :total="total"
          :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next, jumper" @current-change="loadDevices"
          @size-change="handlePageSizeChange" />
      </div>
    </GlassPanel>

    <el-drawer v-model="deviceDetailDrawer" :title="deviceDetailTitle" size="520px" class="device-detail-drawer"
      destroy-on-close>
      <div v-if="detailDevice" class="device-detail">
        <div class="detail-row">
          <div class="detail-item wide"><span>设备 ID</span><b>{{ detailDevice.device_id }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>设备名称</span><b>{{ displayDeviceName(detailDevice) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>注册时间</span><b>{{ detailDevice.register_time || '-' }}</b></div>
        </div>
        <div class="detail-row two">
          <div class="detail-item"><span>类型</span><b>{{ emptyText(detailDevice.device_type) }}</b></div>
          <div class="detail-item"><span>国标版本</span><b>{{ emptyText(detailDevice.gb_version) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide">
            <span>状态</span>
            <b>
              <StatusPill :label="detailDevice.monitor_status === 1 ? '在线' : '离线'"
                :tone="detailDevice.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
            </b>
          </div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>路数</span><b>{{ countText(detailDevice.max_camera) }}</b></div>
        </div>
        <div class="detail-row two">
          <div class="detail-item"><span>（接入）在线</span><b>{{ detailDevice.camera_in_count }}</b></div>
          <div class="detail-item"><span>离线</span><b>{{ detailDevice.camera_off_count }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>型号</span><b>{{ emptyText(detailDevice.model) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>固件版本</span><b>{{ emptyText(detailDevice.firmware) }}</b></div>
        </div>
        <div class="detail-row">
          <div class="detail-item wide"><span>厂家</span><b>{{ emptyText(detailDevice.manufacturer) }}</b></div>
        </div>
      </div>
      <template #footer>
        <el-button @click="deviceDetailDrawer = false">关闭</el-button>
        <el-button v-if="detailDevice" type="primary" @click="openChannelsFromDetail">相机</el-button>
      </template>
    </el-drawer>
  </div>

  <div v-else-if="!selectedDevice" class="page-grid">
    <GlassPanel class="span-12" title="通道树多画面" subtitle="选择当前 Session 节点下的设备通道组成宫格">
      <div class="monitor-head">
        <div class="device-summary">
          <strong>多画面工作台</strong>
          <span>最多 16 路实时直播</span>
        </div>
        <div class="monitor-actions">
          <el-button :loading="multiStopping" @click="collapseMultiTree">一键聚合</el-button>
          <el-button type="danger" plain :loading="multiStopping" @click="stopAllMultiStreams()">停止全部</el-button>
          <el-button type="primary" @click="backToDeviceListFromMulti">返回设备列表</el-button>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-4" title="信令节点" subtitle="先选择节点，再查询设备">
      <div v-loading="listNodeLoading" class="tree-node-list">
        <button v-for="option in sessionNodeOptions" :key="option.node.node_id" type="button" class="tree-node"
          :class="{ active: selectedMultiNodeId === option.node.node_id, offline: option.disabled }"
          :disabled="option.disabled" @click="selectMultiNode(option.node.node_id)">
          <span>{{ option.kindLabel }}</span>
          <b>{{ option.node.node_id }}</b>
          <small>{{ option.statusLabel }}</small>
        </button>
        <el-empty v-if="!sessionNodeOptions.length" description="暂无信令节点" />
      </div>
    </GlassPanel>

    <GlassPanel class="span-8" title="设备与通道" :subtitle="selectedMultiNodeLabel">
      <div v-if="selectedMultiNodeOption" class="tree-workbench">
        <div class="toolbar">
          <el-input v-model="treeDeviceId" style="width: 220px" clearable placeholder="设备 ID" />
          <el-input v-model="treeDeviceName" style="width: 220px" clearable placeholder="设备名称" />
          <el-button type="primary" :loading="treeLoading" @click="queryTreeDevices">查询</el-button>
          <el-button :loading="treeLoading" @click="resetTreeDevices">重置</el-button>
          <el-button type="primary" plain :disabled="!selectedTreeChannelKeys.length" :loading="multiPlaying"
            @click="playSelectedMultiChannels">播放选中</el-button>
        </div>
        <div v-loading="treeLoading" class="tree-device-list">
          <article v-for="device in treeDevices" :key="device.device_id" class="tree-device">
            <header>
              <button type="button" @click="toggleTreeDevice(device)">
                {{ expandedDeviceKeys.includes(device.device_id) ? '收起' : '展开' }}
              </button>
              <div>
                <b>{{ displayDeviceName(device) }}</b>
                <span>{{ device.device_id }}</span>
              </div>
              <StatusPill :label="device.monitor_status === 1 ? '在线' : '离线'"
                :tone="device.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
            </header>
            <div v-if="expandedDeviceKeys.includes(device.device_id)" class="tree-channel-list">
              <el-checkbox v-for="channel in treeChannelsByDevice[device.device_id] || []" :key="channel.channel_id"
                :model-value="selectedTreeChannelKeys.includes(channelKey(channel))" :disabled="!canPlayLive(channel)"
                @change="(checked: boolean) => toggleTreeChannel(channel, checked)">
                <span class="tree-channel-label">
                  <b>{{ displayChannelName(channel) }}</b>
                  <small>{{ channel.channel_id }} · {{ channelStatusText(channel) }}</small>
                </span>
              </el-checkbox>
              <el-empty
                v-if="!treeChannelLoading[device.device_id] && !(treeChannelsByDevice[device.device_id] || []).length"
                description="暂无通道" />
              <div v-if="treeChannelLoading[device.device_id]" class="tree-loading">通道加载中...</div>
            </div>
          </article>
          <el-empty v-if="!treeLoading && !treeDevices.length" description="查询设备后选择通道" />
        </div>
      </div>
      <el-empty v-else description="请选择信令节点" />
    </GlassPanel>

    <GlassPanel class="span-12" title="多画面播放" :subtitle="multiPlayerSubtitle">
      <div class="multi-player">
        <GmvMultiGrid v-model:grid-size="multiGridSize" :cells="multiGridCells" @snapshot="handleMultiSnapshot"
          @ptz="handleMultiPtz" />
      </div>
    </GlassPanel>
  </div>

  <div v-else class="page-grid">
    <GlassPanel class="span-12" title="通道监控" :subtitle="selectedDevice.device_id">
      <div class="monitor-head">
        <div class="device-summary">
          <StatusPill :label="selectedDevice.monitor_status === 1 ? '在线' : '离线'"
            :tone="selectedDevice.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
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
        <a v-for="image in images" :key="image.image_id" class="image-card" :href="image.image_url" target="_blank"
          rel="noreferrer">
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
      <GlassPanel class="span-12" title="相机列表">
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
            <!-- <div class="channel-tags">
              <span>{{ channel.ptz_type || 'PTZ -' }}</span>
              <span>{{ confText(channel.playback_enable, 2, '回放') }}</span>
              <span>{{ confText(channel.snapshot, 2, '抓拍') }}</span>
              <span>{{ confText(channel.biz_enable, 1, '业务') }}</span>
            </div> -->
            <footer class="channel-actions">
              <el-button :disabled="!canPlayLive(channel)" @click="startPlay('preview', channel)">直播</el-button>
              <el-button :disabled="!canPlayback(channel)" @click="startPlay('playback', channel)">回放</el-button>
              <el-button :disabled="!canSnapshot(channel)" :loading="snapshotLoading[channel.channel_id]"
                @click="snapshot(channel)">抓拍</el-button>
              <el-button :disabled="!canViewImages(channel)" @click="openImages(channel)">图集</el-button>
              <el-button :disabled="!canPlayLive(channel)" @click="focusChannelInMultiView(channel)">多画面</el-button>
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

    <el-drawer v-model="configDrawer" title="相机业务配置" size="420px" class="camera-config-drawer" destroy-on-close>
      <el-form :model="configForm" label-width="110px" class="config-form">
        <el-form-item label="设备ID"><el-input v-model="configForm.device_id" disabled /></el-form-item>
        <el-form-item label="通道ID"><el-input v-model="configForm.channel_id" disabled /></el-form-item>
        <el-form-item label="名称"><el-input v-model="configForm.name" disabled /></el-form-item>
        <el-form-item label="别名"><el-input v-model="configForm.alias_name" maxlength="16" clearable /></el-form-item>
        <el-form-item label="排序"><el-input-number v-model="configForm.sort_no" :min="0" :max="999999" /></el-form-item>
        <el-form-item label="云台控制"><el-select v-model="configForm.ptz_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="语音对讲"><el-select v-model="configForm.talk_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="音频"><el-select v-model="configForm.audio_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="抓拍"><el-select v-model="configForm.snapshot"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="录像"><el-select v-model="configForm.record_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="回放"><el-select v-model="configForm.playback_enable"><el-option
              v-for="option in confOptions" :key="option.value" :label="option.label"
              :value="option.value" /></el-select></el-form-item>
        <el-form-item label="告警"><el-select v-model="configForm.alarm_enable"><el-option v-for="option in confOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
        <el-form-item label="业务启用"><el-select v-model="configForm.biz_enable"><el-option v-for="option in bizOptions"
              :key="option.value" :label="option.label" :value="option.value" /></el-select></el-form-item>
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
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import {
  getGbSessionNodeConfig,
  listGbChannelImages,
  listGbChannels,
  listGbDevicePage,
  listNodes,
  sendGbPtz,
  startGbPlayback,
  startGbPreview,
  stopStream,
  takeGbSnapshot,
  updateGbChannel,
  type GbChannelImageInfo,
  type GbChannelInfo,
  type GbChannelPayload,
  type GbDeviceInfo,
  type GbSessionConfigInfo,
  type NodeInfo,
  type StreamSummary,
} from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { GmvMultiGrid, GmvPlayerView, type GmvPtzCommand, type GmvSource } from 'gmv-player';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const monitorMode = ref<'devices' | 'multi'>('devices');
const loading = ref(false);
const channelLoading = ref(false);
const imageLoading = ref(false);
const configSaving = ref(false);
const listNodeLoading = ref(false);
const treeLoading = ref(false);
const multiPlaying = ref(false);
const multiStopping = ref(false);
const deviceName = ref('');
const treeDeviceId = ref('');
const treeDeviceName = ref('');
const devices = ref<GbDeviceInfo[]>([]);
const channels = ref<GbChannelInfo[]>([]);
const treeDevices = ref<GbDeviceInfo[]>([]);
const images = ref<GbChannelImageInfo[]>([]);
const sessionNodes = ref<NodeInfo[]>([]);
const sessionNodeOptions = ref<SessionNodeOption[]>([]);
const selectedListNodeId = ref('');
const selectedMultiNodeId = ref('');
const page = ref(1);
const pageSize = ref(20);
const total = ref(0);
const selectedDevice = ref<GbDeviceInfo>();
const selectedChannel = ref<GbChannelInfo>();
const detailDevice = ref<GbDeviceInfo>();
const lastStream = ref<StreamSummary>();
const lastAction = ref('');
const showImages = ref(false);
const deviceDetailDrawer = ref(false);
const coverDialog = ref(false);
const coverUrl = ref('');
const configDrawer = ref(false);
const snapshotLoading = reactive<Record<string, boolean>>({});
const treeChannelsByDevice = reactive<Record<string, GbChannelInfo[]>>({});
const treeChannelLoading = reactive<Record<string, boolean>>({});
const expandedDeviceKeys = ref<string[]>([]);
const selectedTreeChannelKeys = ref<string[]>([]);
const multiCells = ref<MultiViewCell[]>([]);
const multiGridSize = ref(4);
const configForm = reactive<GbChannelPayload & { device_id?: string }>({ channel_id: '', device_id: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');

type SessionNodeOption = { node: NodeInfo; config?: GbSessionConfigInfo; disabled: boolean; kindLabel: string; statusLabel: string };
type MultiCellStatus = 'idle' | 'online' | 'playing' | 'offline' | 'error';
interface MultiViewCell {
  key: string;
  device_id: string;
  channel_id: string;
  title: string;
  stream?: StreamSummary;
  sources: GmvSource[];
  status: MultiCellStatus;
  error?: string;
}
interface SelectedChannelRef {
  device_id: string;
  channel_id: string;
  title: string;
}

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
const selectedListNodeOption = computed(() => sessionNodeOptions.value.find((item) => item.node.node_id === selectedListNodeId.value));
const selectedMultiNodeOption = computed(() => sessionNodeOptions.value.find((item) => item.node.node_id === selectedMultiNodeId.value));
const selectedMultiNodeLabel = computed(() => selectedMultiNodeOption.value ? listNodeLabel(selectedMultiNodeOption.value) : '未选择信令节点');
const sortedChannels = computed(() => [...channels.value].sort((left, right) => {
  const sortNo = Number(left.sort_no || 0) - Number(right.sort_no || 0);
  return sortNo || displayChannelName(left).localeCompare(displayChannelName(right), 'zh-Hans-CN');
}));
const selectedTreeChannels = computed<SelectedChannelRef[]>(() => selectedTreeChannelKeys.value.flatMap((key) => {
  const [deviceId, channelId] = key.split(':');
  const channel = (treeChannelsByDevice[deviceId] || []).find((item) => item.channel_id === channelId);
  if (!channel || !canPlayLive(channel)) return [];
  return [{
    device_id: channel.device_id,
    channel_id: channel.channel_id,
    title: displayChannelName(channel),
  }];
}));
const selectedChannelTitle = computed(() => selectedChannel.value ? displayChannelName(selectedChannel.value) : '未选择通道');
const deviceDetailTitle = computed(() => detailDevice.value ? '设备详情 · ' + displayDeviceName(detailDevice.value) : '设备详情');
const playerSubtitle = computed(() => lastStream.value?.endpoint || '选择在线通道后播放');
const multiPlayerSubtitle = computed(() => multiCells.value.length ? `运行中 ${multiCells.value.filter((cell) => cell.stream?.state === 'running').length} 路` : '选择通道后播放');
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
const multiGridCells = computed(() => multiCells.value.map((cell) => ({
  sources: cell.sources,
  title: cell.error ? cell.title + ' · ' + cell.error : cell.title,
  deviceId: cell.device_id,
  channelId: cell.channel_id,
  status: cell.status,
  viewers: 1,
  osd: [
    { id: 'channel', text: cell.title, x: 3, y: 5 },
    { id: 'mode', text: '实时直播', x: 3, y: 12 },
  ],
  capabilities: playerCapabilities,
})));

function displayDeviceName(device: GbDeviceInfo) { return device.alias || device.device_id; }
function displayChannelName(channel: GbChannelInfo) { return channel.alias_name || channel.name || channel.channel_id; }
function tableIndex(index: number) { return (page.value - 1) * pageSize.value + index + 1; }
function emptyText(value: unknown) { return value === undefined || value === null || value === '' ? '-' : String(value); }
function countText(value: unknown) { const count = Number(value || 0); return count > 0 ? String(count) : '-'; }
function normalizeKind(value?: string | null) { return (value || '').trim().toLowerCase(); }
function nodeKindLabel(node: NodeInfo) { return (node.kind || node.service || node.config?.service || 'node').toUpperCase(); }
function nodeStatusLabel(disabled: boolean) { return disabled ? '离线' : '在线'; }
function buildSessionNodeOption(node: NodeInfo, config?: GbSessionConfigInfo): SessionNodeOption {
  const disabled = !isNodeOnline(node) || !config?.domain_id;
  return { node, config, disabled, kindLabel: nodeKindLabel(node), statusLabel: nodeStatusLabel(disabled) };
}
function isGbSessionNode(node: NodeInfo) { return normalizeKind(node.kind) === 'session-gb28181' || normalizeKind(node.service) === 'session-gb28181' || normalizeKind(node.protocol) === 'gb28181'; }
function isNodeOnline(node?: NodeInfo) { return !!node && node.connection === 'CONNECTED' && node.scheduling === 'ENABLED'; }
function listNodeLabel(option: SessionNodeOption) { return `${option.kindLabel} · ${option.node.node_id} · ${option.statusLabel}`; }
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
function channelKey(channel: GbChannelInfo) { return `${channel.device_id}:${channel.channel_id}`; }
function streamSources(stream?: StreamSummary): GmvSource[] {
  const endpoint = stream?.endpoint;
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
}
function clearTreeChannelState() {
  expandedDeviceKeys.value = [];
  selectedTreeChannelKeys.value = [];
  for (const key of Object.keys(treeChannelsByDevice)) delete treeChannelsByDevice[key];
  for (const key of Object.keys(treeChannelLoading)) delete treeChannelLoading[key];
}
function clearTreeDeviceState() {
  treeDeviceId.value = '';
  treeDeviceName.value = '';
  treeDevices.value = [];
  clearTreeChannelState();
}
async function openMultiView() {
  monitorMode.value = 'multi';
  selectedDevice.value = undefined;
  showImages.value = false;
  await loadSessionNodes();
}
async function backToDeviceListFromMulti() {
  await stopAllMultiStreams();
  monitorMode.value = 'devices';
  selectedMultiNodeId.value = '';
  clearTreeDeviceState();
  await loadDevices();
}
async function selectMultiNode(nodeId: string) {
  if (selectedMultiNodeId.value === nodeId) return;
  await stopAllMultiStreams();
  selectedMultiNodeId.value = nodeId;
  clearTreeDeviceState();
}
async function queryTreeDevices() {
  const option = selectedMultiNodeOption.value;
  if (!option || option.disabled || !option.config?.domain_id) {
    treeDevices.value = [];
    return;
  }
  treeLoading.value = true;
  try {
    clearTreeChannelState();
    const result = await listGbDevicePage(
      1,
      100,
      option.node.node_id,
      option.config.domain_id,
      treeDeviceId.value,
      treeDeviceName.value,
      true,
    );
    treeDevices.value = result.items;
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '设备查询失败');
  } finally {
    treeLoading.value = false;
  }
}
async function resetTreeDevices() {
  clearTreeDeviceState();
}
function collapseMultiTree() {
  selectedMultiNodeId.value = '';
  clearTreeDeviceState();
}
async function toggleTreeDevice(device: GbDeviceInfo) {
  const index = expandedDeviceKeys.value.indexOf(device.device_id);
  if (index >= 0) {
    expandedDeviceKeys.value.splice(index, 1);
    return;
  }
  expandedDeviceKeys.value.push(device.device_id);
  if (treeChannelsByDevice[device.device_id]) return;
  treeChannelLoading[device.device_id] = true;
  try {
    treeChannelsByDevice[device.device_id] = await listGbChannels(device.device_id);
  } catch (error) {
    treeChannelsByDevice[device.device_id] = [];
    ElMessage.error(error instanceof Error ? error.message : '通道加载失败');
  } finally {
    treeChannelLoading[device.device_id] = false;
  }
}
function toggleTreeChannel(channel: GbChannelInfo, checked: boolean) {
  const key = channelKey(channel);
  if (checked) {
    if (!selectedTreeChannelKeys.value.includes(key)) selectedTreeChannelKeys.value.push(key);
    return;
  }
  selectedTreeChannelKeys.value = selectedTreeChannelKeys.value.filter((item) => item !== key);
}
async function playSelectedMultiChannels() {
  const selected = selectedTreeChannels.value;
  if (!selected.length) {
    ElMessage.warning('请选择可播放通道');
    return;
  }
  if (selected.length > multiGridSize.value) {
    ElMessage.warning('选中通道超过当前宫格数量，请扩大宫格或减少选择');
    return;
  }
  multiPlaying.value = true;
  try {
    await stopAllMultiStreams({ quiet: true });
    const cells = await Promise.all(selected.map(async (channel): Promise<MultiViewCell> => {
      const key = `${channel.device_id}:${channel.channel_id}`;
      try {
        const stream = await startGbPreview(channel.device_id, channel.channel_id, {
          request_id: 'ui-multi-preview-' + Date.now() + '-' + channel.channel_id,
          output_type: 'flv',
        });
        return {
          key,
          device_id: channel.device_id,
          channel_id: channel.channel_id,
          title: channel.title,
          stream,
          sources: streamSources(stream),
          status: stream.state === 'running' ? 'playing' : 'online',
        };
      } catch (error) {
        return {
          key,
          device_id: channel.device_id,
          channel_id: channel.channel_id,
          title: channel.title,
          sources: [],
          status: 'error',
          error: error instanceof Error ? error.message : '播放失败',
        };
      }
    }));
    multiCells.value = cells;
  } finally {
    multiPlaying.value = false;
  }
}
async function stopAllMultiStreams(options: { quiet?: boolean } = {}) {
  if (multiStopping.value) return;
  const streams = multiCells.value.map((cell) => cell.stream).filter((stream): stream is StreamSummary => !!stream?.stream_id);
  multiStopping.value = true;
  try {
    await Promise.allSettled(streams.map((stream) => stopStream(stream.stream_id)));
    multiCells.value = [];
    if (!options.quiet && streams.length) ElMessage.success('多画面已停止');
  } finally {
    multiStopping.value = false;
  }
}
async function stopCurrentStream() {
  const stream = lastStream.value;
  lastStream.value = undefined;
  lastAction.value = '';
  if (stream?.stream_id) await stopStream(stream.stream_id).catch(() => undefined);
}
async function focusChannelInMultiView(channel: GbChannelInfo) {
  const device = selectedDevice.value;
  if (!device) return;
  await stopCurrentStream();
  selectedDevice.value = undefined;
  selectedChannel.value = undefined;
  channels.value = [];
  images.value = [];
  showImages.value = false;
  monitorMode.value = 'multi';
  await loadSessionNodes();
  const targetNodeId = device.session_node_id || selectedListNodeId.value;
  if (targetNodeId && selectedMultiNodeId.value !== targetNodeId) await selectMultiNode(targetNodeId);
  treeDeviceId.value = device.device_id;
  treeDeviceName.value = '';
  await queryTreeDevices();
  const targetDevice = treeDevices.value.find((item) => item.device_id === device.device_id) || treeDevices.value[0];
  if (targetDevice) {
    await toggleTreeDevice(targetDevice);
    const targetChannel = (treeChannelsByDevice[targetDevice.device_id] || []).find((item) => item.channel_id === channel.channel_id);
    if (targetChannel) toggleTreeChannel(targetChannel, true);
  }
  ElMessage.success('已定位到通道树');
}
async function handleMultiSnapshot(event: { index: number }) {
  const cell = multiCells.value[event.index];
  if (!cell) return;
  const channel = (treeChannelsByDevice[cell.device_id] || []).find((item) => item.channel_id === cell.channel_id);
  if (channel) await snapshot(channel);
}
async function handleMultiPtz(event: { index: number; payload: GmvPtzCommand }) {
  const cell = multiCells.value[event.index];
  if (!cell || event.payload.action === 'stop') return;
  try {
    await sendGbPtz(cell.device_id, cell.channel_id);
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '云台控制失败');
  }
}

async function loadSessionNodes() {
  const nodes = (await listNodes()).filter(isGbSessionNode);
  sessionNodes.value = nodes;
  listNodeLoading.value = true;
  try {
    const options = await Promise.all(nodes.map(async (node) => {
      if (!isNodeOnline(node)) return buildSessionNodeOption(node);
      try {
        return buildSessionNodeOption(node, await getGbSessionNodeConfig(node.node_id));
      } catch {
        return buildSessionNodeOption(node);
      }
    }));
    sessionNodeOptions.value = options.sort((left, right) => Number(left.disabled) - Number(right.disabled) || left.node.node_id.localeCompare(right.node.node_id));
    if (!selectedListNodeId.value || !sessionNodeOptions.value.some((item) => item.node.node_id === selectedListNodeId.value && !item.disabled)) {
      selectedListNodeId.value = sessionNodeOptions.value.find((item) => !item.disabled)?.node.node_id || '';
    }
  } finally {
    listNodeLoading.value = false;
  }
}
async function loadDevices() {
  loading.value = true;
  try {
    await loadSessionNodes();
    const option = selectedListNodeOption.value;
    if (!option || option.disabled || !option.config?.domain_id) {
      devices.value = [];
      total.value = 0;
      return;
    }
    const result = await listGbDevicePage(
      page.value,
      pageSize.value,
      option.node.node_id,
      option.config.domain_id,
      '',
      deviceName.value,
      true,
    );
    devices.value = result.items;
    total.value = result.total;
    page.value = result.page;
    pageSize.value = result.page_size;
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '设备列表加载失败');
  } finally {
    loading.value = false;
  }
}
async function queryDevices() { page.value = 1; await loadDevices(); }
async function resetDevices() { deviceName.value = ''; page.value = 1; await loadDevices(); }
async function handlePageSizeChange() { page.value = 1; await loadDevices(); }
async function handleListNodeChange() { page.value = 1; await loadDevices(); }
function openDeviceDetail(device: GbDeviceInfo) {
  detailDevice.value = device;
  deviceDetailDrawer.value = true;
}
async function openChannelsFromDetail() {
  if (!detailDevice.value) return;
  const device = detailDevice.value;
  deviceDetailDrawer.value = false;
  await openChannels(device);
}
async function openChannels(device: GbDeviceInfo) {
  await stopCurrentStream();
  selectedDevice.value = device;
  selectedChannel.value = undefined;
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
async function backToDevices() {
  await stopCurrentStream();
  selectedDevice.value = undefined;
  selectedChannel.value = undefined;
  channels.value = [];
  images.value = [];
  showImages.value = false;
}
async function startPlay(kind: 'preview' | 'playback', channel: GbChannelInfo) {
  await stopCurrentStream();
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
onBeforeUnmount(() => {
  void stopAllMultiStreams({ quiet: true });
  void stopCurrentStream();
});
</script>

<style scoped>
.node-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
}

.node-status {
  color: var(--cyan);
  font-size: 12px;
}

.node-option.offline {
  color: var(--muted);
}

.node-option.offline .node-status {
  color: var(--muted);
}

.pagination-bar {
  display: flex;
  justify-content: flex-end;
  padding-top: 14px;
}

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
  grid-template-columns: repeat(6, minmax(0, 1fr));
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

.tree-node-list,
.tree-device-list,
.tree-workbench {
  display: grid;
  gap: 12px;
}

.tree-node {
  display: grid;
  gap: 5px;
  width: 100%;
  padding: 12px;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
  color: var(--text);
  text-align: left;
  cursor: pointer;
}

.tree-node.active {
  border-color: rgba(34, 211, 238, .62);
  background: rgba(34, 211, 238, .08);
}

.tree-node.offline {
  color: var(--muted);
  cursor: not-allowed;
}

.tree-node span,
.tree-node small,
.tree-device header span,
.tree-channel-label small,
.tree-loading {
  color: var(--muted);
  font-size: 12px;
}

.tree-node b,
.tree-device header b,
.tree-channel-label b {
  overflow-wrap: anywhere;
}

.tree-device {
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
}

.tree-device header {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  padding: 12px;
}

.tree-device header button {
  height: 30px;
  border: 1px solid rgba(100, 203, 255, .22);
  border-radius: 6px;
  background: rgba(255, 255, 255, .04);
  color: var(--cyan);
  cursor: pointer;
}

.tree-device header div,
.tree-channel-label {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.tree-channel-list {
  display: grid;
  gap: 8px;
  padding: 0 12px 12px 48px;
}

.tree-channel-list :deep(.el-checkbox) {
  height: auto;
  align-items: flex-start;
  white-space: normal;
}

.multi-player {
  min-height: 620px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
  padding: 12px;
}

.multi-player :deep(.multi-grid) {
  height: 100%;
  min-height: 596px;
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

.device-detail {
  display: grid;
  gap: 10px;
}

.detail-row {
  display: grid;
  grid-template-columns: 1fr;
  gap: 10px;
}

.detail-row.two {
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
}

.detail-item {
  display: grid;
  grid-template-columns: 86px minmax(0, 1fr);
  gap: 10px;
  align-items: center;
  min-width: 0;
  padding: 12px 14px;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
}

.detail-item.wide {
  grid-template-columns: 96px minmax(0, 1fr);
}

.detail-item span {
  min-width: 0;
  color: var(--muted);
  font-size: 12px;
  white-space: nowrap;
}

.detail-item b {
  display: flex;
  min-height: 22px;
  align-items: center;
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 14px;
  font-weight: 700;
}

:deep(.device-detail-drawer),
:deep(.camera-config-drawer) {
  border-left: 1px solid var(--line);
  background: linear-gradient(145deg, rgba(13, 29, 58, .98), rgba(7, 16, 34, .96)) !important;
  color: var(--text);
  box-shadow: var(--shadow);
}

:deep(.device-detail-drawer .el-drawer__header),
:deep(.camera-config-drawer .el-drawer__header) {
  margin-bottom: 0;
  padding: 18px 20px 14px;
  border-bottom: 1px solid rgba(100, 203, 255, .12);
  color: var(--text);
}

:deep(.device-detail-drawer .el-drawer__title),
:deep(.camera-config-drawer .el-drawer__title) {
  color: var(--text);
  font-size: 16px;
  font-weight: 800;
}

:deep(.device-detail-drawer .el-drawer__body),
:deep(.camera-config-drawer .el-drawer__body) {
  padding: 18px 20px;
}

:deep(.device-detail-drawer .el-drawer__footer),
:deep(.camera-config-drawer .el-drawer__footer) {
  padding: 12px 20px 18px;
  border-top: 1px solid rgba(100, 203, 255, .12);
}

:deep(.camera-config-drawer .el-form-item) {
  margin-bottom: 18px;
}

:deep(.camera-config-drawer .el-form-item__label) {
  color: var(--muted) !important;
  font-weight: 700;
}

:deep(.camera-config-drawer .el-input__wrapper),
:deep(.camera-config-drawer .el-select__wrapper) {
  background: rgba(4, 12, 28, .62) !important;
  border-color: rgba(105, 205, 255, .22);
}

:deep(.camera-config-drawer .el-input.is-disabled .el-input__wrapper) {
  background: rgba(9, 18, 38, .72) !important;
  border-color: rgba(100, 203, 255, .12);
}

:deep(.camera-config-drawer .el-input.is-disabled .el-input__inner) {
  color: var(--muted) !important;
  -webkit-text-fill-color: var(--muted) !important;
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

  .detail-row.two {
    grid-template-columns: 1fr;
  }
}
</style>
