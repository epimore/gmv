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
        <!-- <el-button :loading="loading" @click="loadDevices">刷新</el-button> -->
        <el-button type="primary" plain @click="openMultiView">多画面工作台</el-button>
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
            <el-button type="primary" link @click="openDeviceDetail(row)">查看</el-button>
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
    <GlassPanel class="span-12">
      <div class="monitor-head">
        <div class="device-summary">
          <strong>多画面工作台</strong>
          <span>最多 16 路实时直播</span>
        </div>
        <div class="monitor-actions">
          <el-select v-model="selectedMultiNodeId" filterable placeholder="选择 Session 节点" class="multi-node-select"
            :loading="listNodeLoading" @change="selectMultiNode">
            <el-option v-for="option in sessionNodeOptions" :key="option.node.node_id" :label="listNodeLabel(option)"
              :value="option.node.node_id" :disabled="option.disabled">
              <div class="node-option" :class="{ offline: option.disabled }">
                <span>{{ option.kindLabel }} · {{ option.node.node_id }}</span>
                <span class="node-status">{{ option.statusLabel }}</span>
              </div>
            </el-option>
          </el-select>
          <el-button type="danger" plain :loading="multiStopping" @click="stopAllMultiStreams()">停止全部</el-button>
          <el-button type="primary" @click="backToDeviceListFromMulti">返回设备列表</el-button>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-5" title="设备与通道">
      <div v-if="selectedMultiNodeOption" class="tree-workbench">
        <div class="toolbar">
          <el-input v-model="treeDeviceId" style="width: 220px" clearable placeholder="设备 ID" />
          <el-input v-model="treeDeviceName" style="width: 220px" clearable placeholder="设备名称" />
          <el-button type="primary" :loading="treeLoading" @click="searchTreeDevices">查询</el-button>
          <el-button :loading="treeLoading" @click="resetTreeDevices">重置</el-button>
        </div>
        <div v-loading="treeLoading" class="tree-device-list">
          <el-tree class="device-channel-tree" :data="treeDeviceNodes" :props="treeProps" node-key="key" lazy
            :load="loadTreeNode" accordion :expand-on-click-node="true" :highlight-current="false">
            <template #default="{ data }">
              <div v-if="data.kind === 'device'" class="tree-device-node">
                <div class="tree-device-title">
                  <b>{{ data.device.device_id }} · {{ data.label }}</b>
                </div>
                <StatusPill :label="data.device.monitor_status === 1 ? '在线' : '离线'"
                  :tone="data.device.monitor_status === 1 ? 'ONLINE' : 'OFFLINE'" />
              </div>
              <el-checkbox v-else class="tree-channel-node" :model-value="selectedTreeChannelKeys.includes(data.key)"
                :disabled="!canPlayLive(data.channel)" @click.stop
                @change="(checked: boolean) => toggleTreeChannel(data.channel, checked)">
                <span class="tree-channel-label">
                  <b>{{ data.label }}</b>
                  <small>{{ data.channel.channel_id }} · {{ channelStatusText(data.channel) }}</small>
                </span>
              </el-checkbox>
            </template>
          </el-tree>
          <el-empty v-if="!treeLoading && !treeDevices.length" description="暂无设备" />
        </div>
        <div class="pagination-bar tree-pagination">
          <el-pagination v-model:current-page="treePage" v-model:page-size="treePageSize" :total="treeTotal"
            :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next" @current-change="queryTreeDevices"
            @size-change="handleTreePageSizeChange" />
        </div>
      </div>
      <el-empty v-else description="请选择信令节点" />
    </GlassPanel>

    <GlassPanel class="span-7" title="已选通道" :subtitle="selectedTreeChannelSubtitle">
      <div class="selected-channel-panel">
        <div class="selected-channel-list">
          <article v-for="(channel, index) in selectedTreeChannels" :key="channel.device_id + ':' + channel.channel_id"
            class="selected-channel-item" :class="{ dragging: draggingTreeChannelIndex === index }" draggable="true"
            @dragstart="handleSelectedChannelDragStart(index)" @dragover.prevent @drop="handleSelectedChannelDrop(index)"
            @dragend="handleSelectedChannelDragEnd">
            <div>
              <el-tooltip :content="selectedChannelTooltip(channel)" placement="top">
                <b>{{ index + 1 }}. {{ channel.device_id }} · {{ channel.channel_id }}</b>
              </el-tooltip>
            </div>
            <el-button type="danger" link @click="removeTreeChannel(channel)">移除</el-button>
          </article>
          <el-empty v-if="!selectedTreeChannels.length" description="暂无已选通道" />
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-12" title="多画面播放" :subtitle="multiPlayerSubtitle">
      <template #action>
        <div class="player-controls-toggle">
          <span>操作控件</span>
          <el-switch v-model="playerControlsVisible" inline-prompt active-text="显示" inactive-text="隐藏" />
        </div>
      </template>
      <div class="multi-player">
        <GmvMultiGrid v-model:grid-size="multiGridSize" :cells="multiGridCells"
          :controls-visible="playerControlsVisible" @snapshot="handleMultiSnapshot" @ptz="handleMultiPtz"
          @close="handleMultiClose" @reorder="handleMultiReorder" />
        <div v-if="multiPageCount > 1" class="multi-pagination">
          <el-pagination v-model:current-page="multiPage" :page-size="multiGridSize" :total="multiCells.length"
            layout="total, prev, pager, next" />
        </div>
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
              <el-button :disabled="!canPlayLive(channel) || playerRequesting"
                :loading="isPlayRequesting('preview', channel)" @click="startPlay('preview', channel)">直播</el-button>
              <el-button :disabled="!canPlayback(channel) || playerRequesting"
                :loading="isPlayRequesting('playback', channel)" @click="startPlay('playback', channel)">回放</el-button>
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

    </template>

    <el-dialog v-model="coverDialog" title="封面快照" width="720px">
      <img v-if="coverUrl" class="cover-large" :src="coverUrl" alt="封面快照" />
      <el-empty v-else description="暂无封面" />
    </el-dialog>

    <el-dialog v-model="playerDialog" :title="playerDialogTitle" width="960px" class="monitor-player-dialog"
      destroy-on-close @close="stopCurrentStream">
      <div v-if="selectedChannel" class="monitor-player">
        <div class="monitor-player-toolbar">
          <span>{{ selectedChannelTitle }}</span>
          <el-switch v-model="playerControlsVisible" inline-prompt active-text="显示" inactive-text="隐藏" />
        </div>
        <div class="monitor-player-stage">
          <GmvPlayerView :sources="playerSources" :device-id="selectedChannel?.device_id"
            :channel-id="selectedChannel?.channel_id" :title="selectedChannelTitle" :status="playerStatus" :viewers="1"
            :poster="playerPoster" :osd="playerOsd" :capabilities="playerCapabilities"
            :controls-visible="playerControlsVisible"
            @snapshot="selectedChannel && snapshot(selectedChannel)" @ptz="handlePlayerPtz" />
          <div v-if="showDefaultWaitingCover" class="player-waiting-cover" aria-hidden="true">
            <span class="waiting-ring"></span>
            <span class="waiting-scan"></span>
          </div>
          <div v-if="playerRequesting" class="player-loading-badge">播放创建中...</div>
        </div>
      </div>
      <el-empty v-else description="选择在线通道后播放" />
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
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { ElMessage } from 'element-plus';
import {
  errorMessage,
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
  type GbPtzPayload,
  type GbSessionConfigInfo,
  type NodeInfo,
  type StreamSummary,
} from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { GmvMultiGrid, GmvPlayerView, type GmvCodec, type GmvPtzCommand, type GmvSource, type GmvViewCapabilities } from 'gmv-player';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const monitorMode = ref<'devices' | 'multi'>('devices');
const loading = ref(false);
const channelLoading = ref(false);
const imageLoading = ref(false);
const configSaving = ref(false);
const listNodeLoading = ref(false);
const treeLoading = ref(false);
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
const treePage = ref(1);
const treePageSize = ref(20);
const treeTotal = ref(0);
const selectedDevice = ref<GbDeviceInfo>();
const selectedChannel = ref<GbChannelInfo>();
const detailDevice = ref<GbDeviceInfo>();
const lastStream = ref<StreamSummary>();
const lastAction = ref('');
const showImages = ref(false);
const deviceDetailDrawer = ref(false);
const coverDialog = ref(false);
const coverUrl = ref('');
const playerDialog = ref(false);
const playerControlsVisible = ref(true);
const playerRequesting = ref(false);
const pendingPlayKey = ref('');
const configDrawer = ref(false);
const snapshotLoading = reactive<Record<string, boolean>>({});
const treeChannelsByDevice = reactive<Record<string, GbChannelInfo[]>>({});
const treeChannelLoading = reactive<Record<string, boolean>>({});
const selectedTreeChannelKeys = ref<string[]>([]);
const selectedTreeChannelItems = ref<SelectedChannelRef[]>([]);
const draggingTreeChannelIndex = ref<number>();
const multiCells = ref<MultiViewCell[]>([]);
const multiGridSize = ref(4);
const multiPage = ref(1);
const multiPlayVersions = reactive<Record<string, number>>({});
let stopCurrentStreamTask: Promise<void> | undefined;
let playRequestSeq = 0;
const configForm = reactive<GbChannelPayload & { device_id?: string }>({ channel_id: '', device_id: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');

type SessionNodeOption = { node: NodeInfo; config?: GbSessionConfigInfo; disabled: boolean; kindLabel: string; statusLabel: string };
type MultiCellStatus = 'idle' | 'online' | 'playing' | 'offline' | 'reconnecting' | 'error';
interface MultiViewCell {
  key: string;
  device_id: string;
  channel_id: string;
  title: string;
  poster?: string;
  stream?: StreamSummary;
  sources: GmvSource[];
  status: MultiCellStatus;
  error?: string;
}
interface SelectedChannelRef {
  device_id: string;
  channel_id: string;
  title: string;
  poster?: string;
  device_title: string;
  status_text: string;
}
type TreeNodeData =
  | { key: string; label: string; kind: 'device'; device: GbDeviceInfo; leaf: false }
  | { key: string; label: string; kind: 'channel'; channel: GbChannelInfo; leaf: true };

const confOptions = [
  { label: '启用', value: 1 },
  { label: '禁用', value: 0 },
  { label: '设备不支持', value: 2 },
];
const bizOptions = [
  { label: '启用', value: 1 },
  { label: '禁用', value: 0 },
];
const multiPlayerCapabilities: GmvViewCapabilities = {
  ptz: true,
  presets: false,
  snapshot: true,
  record: false,
  playback: true,
  audio: false,
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
const selectedTreeChannels = computed<SelectedChannelRef[]>(() => selectedTreeChannelItems.value);
const selectedTreeChannelSubtitle = computed(() => `${selectedTreeChannels.value.length}/16`);
const treeProps = { label: 'label', isLeaf: 'leaf' };
const treeDeviceNodes = computed<TreeNodeData[]>(() => treeDevices.value.map((device) => ({
  key: device.device_id,
  label: displayDeviceName(device),
  kind: 'device',
  device,
  leaf: false,
})));
const selectedChannelTitle = computed(() => selectedChannel.value ? displayChannelName(selectedChannel.value) : '未选择通道');
const deviceDetailTitle = computed(() => detailDevice.value ? '设备详情 · ' + displayDeviceName(detailDevice.value) : '设备详情');
const playerDialogTitle = computed(() => lastAction.value ? lastAction.value + ' · ' + selectedChannelTitle.value : '播放窗口');
const multiPlayerSubtitle = computed(() => multiCells.value.length ? `实时直播 · 运行中 ${multiCells.value.filter((cell) => cell.stream?.state === 'running').length} 路` : '实时直播 · 选择通道后播放');
const multiPageCount = computed(() => Math.max(1, Math.ceil(multiCells.value.length / multiGridSize.value)));
const multiVisibleStart = computed(() => (multiPage.value - 1) * multiGridSize.value);
const playerStatus = computed(() => lastStream.value?.state === 'running' ? 'playing' : selectedChannel.value && channelOnline(selectedChannel.value) ? 'online' : 'idle');
const playerPoster = computed(() => selectedChannel.value?.pic_url || undefined);
const showDefaultWaitingCover = computed(() => playerRequesting.value && !playerPoster.value);
const playerCapabilities = computed<GmvViewCapabilities>(() => {
  const channel = selectedChannel.value;
  return {
    ptz: channel ? canPtz(channel) : false,
    presets: false,
    snapshot: channel ? canSnapshot(channel) : false,
    record: false,
    playback: channel ? lastAction.value === '历史回放' && canPlayback(channel) : false,
    audio: channel ? canAudio(channel) : false,
    talk: false,
    streamSwitch: false,
    aiOverlay: false,
  };
});
const playerOsd = computed(() => [
  { id: 'channel', text: selectedChannelTitle.value, x: 3, y: 5 },
  { id: 'mode', text: lastAction.value || 'monitor', x: 3, y: 12 },
]);
const playerSources = computed<GmvSource[]>(() => {
  const endpoint = lastStream.value?.endpoint;
  if (!endpoint) return [];
  const protocol = streamProtocol(endpoint);
  const codec = streamCodec(lastStream.value);
  return [{
    protocol,
    codec,
    url: endpoint,
    mimeCodec: fmp4MimeCodec(codec),
    hasAudio: selectedChannel.value ? canAudio(selectedChannel.value) : false,
    label: streamSourceLabel(codec, !!selectedChannel.value && canAudio(selectedChannel.value)),
    priority: 1,
  }];
});
const multiGridCells = computed(() => multiCells.value.slice(multiVisibleStart.value, multiVisibleStart.value + multiGridSize.value).map((cell) => ({
  sources: cell.sources,
  title: cell.error ? cell.title + ' · ' + cell.error : cell.title,
  deviceId: cell.device_id,
  channelId: cell.channel_id,
  status: cell.status,
  viewers: 1,
  poster: cell.poster,
  osd: [
    { id: 'channel', text: cell.title, x: 3, y: 5 },
    { id: 'mode', text: '实时直播', x: 3, y: 12 },
  ],
  capabilities: multiPlayerCapabilities,
})));

watch([multiGridSize, () => multiCells.value.length], () => {
  if (multiPage.value > multiPageCount.value) multiPage.value = multiPageCount.value;
  if (multiPage.value < 1) multiPage.value = 1;
});

function displayDeviceName(device: GbDeviceInfo) { return device.alias || device.device_id; }
function displayChannelName(channel: GbChannelInfo) { return channel.alias_name || channel.name || channel.channel_id; }
function selectedChannelTooltip(channel: SelectedChannelRef) { return `${channel.device_title} · ${channel.title}`; }
function tableIndex(index: number) { return (page.value - 1) * pageSize.value + index + 1; }
function emptyText(value: unknown) { return value === undefined || value === null || value === '' ? '-' : String(value); }
function countText(value: unknown) { const count = Number(value || 0); return count > 0 ? String(count) : '-'; }
function normalizeKind(value?: string | null) { return (value || '').trim().toLowerCase(); }
function nodeKindLabel(node: NodeInfo) { return (node.kind || node.service || node.config?.service || 'node').toUpperCase(); }
function nodeStatusLabel(disabled: boolean, reason?: string) { return reason || (disabled ? '离线' : '在线'); }
function buildSessionNodeOption(node: NodeInfo, config?: GbSessionConfigInfo, disabledReason?: string): SessionNodeOption {
  const disabled = !isNodeOnline(node) || !config?.domain_id;
  const reason = disabledReason || (isNodeOnline(node) && !config?.domain_id ? '缺少 domain 配置' : undefined);
  return { node, config, disabled, kindLabel: nodeKindLabel(node), statusLabel: nodeStatusLabel(disabled, reason) };
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
function canPtz(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.ptz_enable); }
function canAudio(channel: GbChannelInfo) { return channelOnline(channel) && bizEnabled(channel) && confEnabled(channel.audio_enable); }
function canViewImages(channel: GbChannelInfo) { return bizEnabled(channel); }
function playRequestKey(kind: 'preview' | 'playback', channel: GbChannelInfo) { return `${kind}:${channel.device_id}:${channel.channel_id}`; }
function isPlayRequesting(kind: 'preview' | 'playback', channel: GbChannelInfo) { return playerRequesting.value && pendingPlayKey.value === playRequestKey(kind, channel); }
function streamProtocol(endpoint: string): GmvSource['protocol'] {
  const path = endpoint.split('?')[0].toLowerCase();
  if (path.endsWith('.fmp4')) return 'fmp4';
  if (path.endsWith('.m3u8')) return 'hls';
  return 'flv';
}
function streamCodec(stream?: StreamSummary): GmvCodec | undefined {
  const codec = (stream?.video_codec || '').trim().toLowerCase();
  if (codec === 'h264' || codec === 'h.264' || codec === 'avc' || codec === 'avc1') return 'h264';
  if (codec === 'h265' || codec === 'h.265' || codec === 'hevc' || codec === 'hev1' || codec === 'hvc1') return 'h265';
  return undefined;
}
function fmp4MimeCodec(codec?: GmvCodec) {
  if (codec === 'h264') return 'video/mp4; codecs="avc1.42E01E, mp4a.40.2"';
  if (codec === 'h265') return 'video/mp4; codecs="hvc1.1.6.L123.B0, mp4a.40.2"';
  return undefined;
}
function streamSourceLabel(codec: GmvCodec | undefined, hasAudio: boolean) {
  return `默认${hasAudio ? '音视频' : '静音'} · ${codec?.toUpperCase() || 'AUTO'}`;
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
  const codec = streamCodec(stream);
  return [{
    protocol,
    codec,
    url: endpoint,
    mimeCodec: fmp4MimeCodec(codec),
    hasAudio: false,
    label: streamSourceLabel(codec, false),
    priority: 1,
  }];
}
function clearTreeLoadedChannelState() {
  for (const key of Object.keys(treeChannelsByDevice)) delete treeChannelsByDevice[key];
  for (const key of Object.keys(treeChannelLoading)) delete treeChannelLoading[key];
}
function clearTreeChannelState() {
  clearTreeLoadedChannelState();
  selectedTreeChannelKeys.value = [];
  selectedTreeChannelItems.value = [];
}
function clearTreeDeviceState() {
  treeDeviceId.value = '';
  treeDeviceName.value = '';
  treeDevices.value = [];
  treePage.value = 1;
  treeTotal.value = 0;
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
  await stopAllMultiStreams();
  selectedMultiNodeId.value = nodeId;
  clearTreeDeviceState();
  await queryTreeDevices();
}
async function searchTreeDevices() {
  treePage.value = 1;
  await queryTreeDevices();
}
async function queryTreeDevices() {
  const option = selectedMultiNodeOption.value;
  if (!option || option.disabled || !option.config?.domain_id) {
    treeDevices.value = [];
    treeTotal.value = 0;
    return;
  }
  treeLoading.value = true;
  try {
    clearTreeLoadedChannelState();
    const result = await listGbDevicePage(
      treePage.value,
      treePageSize.value,
      option.node.node_id,
      option.config.domain_id,
      treeDeviceId.value,
      treeDeviceName.value,
      true,
    );
    treeDevices.value = result.items;
    treeTotal.value = result.total;
    treePage.value = result.page;
    treePageSize.value = result.page_size;
  } catch (error) {
    ElMessage.error(errorMessage(error, '设备查询失败'));
  } finally {
    treeLoading.value = false;
  }
}
async function resetTreeDevices() {
  await stopAllMultiStreams({ quiet: true });
  clearTreeDeviceState();
  if (selectedMultiNodeOption.value) await queryTreeDevices();
}
async function handleTreePageSizeChange() {
  treePage.value = 1;
  await queryTreeDevices();
}
async function loadTreeDeviceChannels(device: GbDeviceInfo) {
  if (treeChannelsByDevice[device.device_id]) return treeChannelsByDevice[device.device_id];
  treeChannelLoading[device.device_id] = true;
  try {
    treeChannelsByDevice[device.device_id] = await listGbChannels(device.device_id);
  } catch (error) {
    treeChannelsByDevice[device.device_id] = [];
    ElMessage.error(errorMessage(error, '通道加载失败'));
  } finally {
    treeChannelLoading[device.device_id] = false;
  }
  return treeChannelsByDevice[device.device_id];
}
async function loadTreeNode(node: { level: number; data?: TreeNodeData }, resolve: (data: TreeNodeData[]) => void) {
  const data = node.data;
  if (!data || data.kind !== 'device') {
    resolve([]);
    return;
  }
  const channels = await loadTreeDeviceChannels(data.device);
  resolve(channels.map((channel) => ({
    key: channelKey(channel),
    label: displayChannelName(channel),
    kind: 'channel',
    channel,
    leaf: true,
  })));
}
async function toggleTreeChannel(channel: GbChannelInfo, checked: boolean) {
  const key = channelKey(channel);
  if (checked) {
    if (selectedTreeChannelKeys.value.includes(key)) return;
    if (selectedTreeChannelKeys.value.length >= 16) {
      ElMessage.warning('最多选择 16 个通道');
      return;
    }
    const device = treeDevices.value.find((item) => item.device_id === channel.device_id);
    selectedTreeChannelKeys.value.push(key);
    selectedTreeChannelItems.value.push({
      device_id: channel.device_id,
      channel_id: channel.channel_id,
      title: displayChannelName(channel),
      poster: channel.pic_url || undefined,
      device_title: device ? displayDeviceName(device) : channel.device_id,
      status_text: channelStatusText(channel),
    });
    await startSelectedMultiChannel(selectedTreeChannelItems.value[selectedTreeChannelItems.value.length - 1]);
    return;
  }
  await stopMultiCell(key);
}
async function removeTreeChannel(channel: SelectedChannelRef) {
  await stopMultiCell(selectedChannelKey(channel));
}
function syncSelectedTreeChannelKeys() {
  selectedTreeChannelKeys.value = selectedTreeChannelItems.value.map((item) => `${item.device_id}:${item.channel_id}`);
}
function syncSelectedTreeChannelsOrderFromCells() {
  const order = new Map(multiCells.value.map((cell, index) => [cell.key, index]));
  selectedTreeChannelItems.value = [...selectedTreeChannelItems.value].sort((left, right) => {
    const leftOrder = order.get(selectedChannelKey(left)) ?? Number.MAX_SAFE_INTEGER;
    const rightOrder = order.get(selectedChannelKey(right)) ?? Number.MAX_SAFE_INTEGER;
    return leftOrder - rightOrder;
  });
  syncSelectedTreeChannelKeys();
}
function syncMultiCellsOrder() {
  const order = new Map(selectedTreeChannelItems.value.map((item, index) => [`${item.device_id}:${item.channel_id}`, index]));
  multiCells.value = [...multiCells.value].sort((left, right) => {
    const leftOrder = order.get(`${left.device_id}:${left.channel_id}`) ?? Number.MAX_SAFE_INTEGER;
    const rightOrder = order.get(`${right.device_id}:${right.channel_id}`) ?? Number.MAX_SAFE_INTEGER;
    return leftOrder - rightOrder;
  });
}
function handleSelectedChannelDragStart(index: number) {
  draggingTreeChannelIndex.value = index;
}
function handleSelectedChannelDrop(targetIndex: number) {
  const sourceIndex = draggingTreeChannelIndex.value;
  draggingTreeChannelIndex.value = undefined;
  if (sourceIndex === undefined || sourceIndex === targetIndex) return;
  const items = [...selectedTreeChannelItems.value];
  const [item] = items.splice(sourceIndex, 1);
  if (!item) return;
  items.splice(targetIndex, 0, item);
  selectedTreeChannelItems.value = items;
  syncSelectedTreeChannelKeys();
  syncMultiCellsOrder();
}
function handleSelectedChannelDragEnd() {
  draggingTreeChannelIndex.value = undefined;
}
function bumpMultiPlayVersion(key: string) {
  multiPlayVersions[key] = (multiPlayVersions[key] || 0) + 1;
  return multiPlayVersions[key];
}
function selectedChannelKey(channel: SelectedChannelRef) { return `${channel.device_id}:${channel.channel_id}`; }
function upsertMultiCell(cell: MultiViewCell) {
  const index = multiCells.value.findIndex((item) => item.key === cell.key);
  if (index >= 0) {
    multiCells.value.splice(index, 1, cell);
  } else {
    multiCells.value.push(cell);
    multiPage.value = Math.ceil(multiCells.value.length / multiGridSize.value);
  }
  syncMultiCellsOrder();
}
async function startSelectedMultiChannel(channel?: SelectedChannelRef) {
  if (!channel) return;
  const key = selectedChannelKey(channel);
  const version = bumpMultiPlayVersion(key);
  upsertMultiCell({
    key,
    device_id: channel.device_id,
    channel_id: channel.channel_id,
    title: channel.title,
    poster: channel.poster,
    sources: [],
    status: 'reconnecting',
  });
  try {
    const stream = await startGbPreview(channel.device_id, channel.channel_id, {
      request_id: 'ui-multi-preview-' + Date.now() + '-' + channel.channel_id,
      output_type: 'flv',
    });
    if (multiPlayVersions[key] !== version || !selectedTreeChannelKeys.value.includes(key)) return;
    upsertMultiCell({
      key,
      device_id: channel.device_id,
      channel_id: channel.channel_id,
      title: channel.title,
      poster: channel.poster,
      stream,
      sources: streamSources(stream),
      status: stream.state === 'running' ? 'playing' : 'online',
    });
  } catch (error) {
    if (multiPlayVersions[key] !== version || !selectedTreeChannelKeys.value.includes(key)) return;
    upsertMultiCell({
      key,
      device_id: channel.device_id,
      channel_id: channel.channel_id,
      title: channel.title,
      poster: channel.poster,
      sources: [],
      status: 'error',
      error: errorMessage(error, '播放失败'),
    });
  }
}
async function stopMultiCell(key: string, options: { removeSelection?: boolean } = {}) {
  const removeSelection = options.removeSelection !== false;
  bumpMultiPlayVersion(key);
  const cell = multiCells.value.find((item) => item.key === key);
  multiCells.value = multiCells.value.filter((item) => item.key !== key);
  if (removeSelection) {
    selectedTreeChannelKeys.value = selectedTreeChannelKeys.value.filter((item) => item !== key);
    selectedTreeChannelItems.value = selectedTreeChannelItems.value.filter((item) => selectedChannelKey(item) !== key);
  }
  if (cell?.stream?.stream_id) await stopStream(cell.stream.stream_id).catch(() => undefined);
}
async function stopAllMultiStreams(options: { quiet?: boolean } = {}) {
  if (multiStopping.value) return;
  const streams = multiCells.value.map((cell) => cell.stream).filter((stream): stream is StreamSummary => !!stream?.stream_id);
  multiStopping.value = true;
  try {
    for (const cell of multiCells.value) bumpMultiPlayVersion(cell.key);
    await Promise.allSettled(streams.map((stream) => stopStream(stream.stream_id)));
    multiCells.value = [];
    selectedTreeChannelKeys.value = [];
    selectedTreeChannelItems.value = [];
    multiPage.value = 1;
    if (!options.quiet && streams.length) ElMessage.success('多画面已停止');
  } finally {
    multiStopping.value = false;
  }
}
async function stopCurrentStream(options: { closeDialog?: boolean; clearAction?: boolean; cancelPending?: boolean } = {}) {
  if (stopCurrentStreamTask) return stopCurrentStreamTask;
  const closeDialog = options.closeDialog !== false;
  const clearAction = options.clearAction !== false;
  const cancelPending = options.cancelPending !== false;
  stopCurrentStreamTask = (async () => {
    const stream = lastStream.value;
    if (cancelPending) {
      playRequestSeq += 1;
      playerRequesting.value = false;
      pendingPlayKey.value = '';
    }
    if (closeDialog) playerDialog.value = false;
    lastStream.value = undefined;
    if (clearAction) lastAction.value = '';
    if (stream?.stream_id) await stopStream(stream.stream_id).catch(() => undefined);
  })().finally(() => {
    stopCurrentStreamTask = undefined;
  });
  return stopCurrentStreamTask;
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
    await loadTreeDeviceChannels(targetDevice);
    const targetChannel = (treeChannelsByDevice[targetDevice.device_id] || []).find((item) => item.channel_id === channel.channel_id);
    if (targetChannel) await toggleTreeChannel(targetChannel, true);
  }
  ElMessage.success('已定位到通道树');
}
function multiCellAtVisibleIndex(index: number) {
  return multiCells.value[multiVisibleStart.value + index];
}
async function handleMultiSnapshot(event: { index: number }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  const channel = (treeChannelsByDevice[cell.device_id] || []).find((item) => item.channel_id === cell.channel_id);
  if (channel) await snapshot(channel);
}
async function handleMultiClose(event: { index: number }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  await stopMultiCell(cell.key);
}
function handleMultiReorder(event: { sourceIndex: number; targetIndex: number }) {
  const sourceIndex = multiVisibleStart.value + event.sourceIndex;
  const targetIndex = multiVisibleStart.value + event.targetIndex;
  if (sourceIndex === targetIndex || !multiCells.value[sourceIndex] || !multiCells.value[targetIndex]) return;
  const cells = [...multiCells.value];
  const [cell] = cells.splice(sourceIndex, 1);
  if (!cell) return;
  cells.splice(targetIndex, 0, cell);
  multiCells.value = cells;
  syncSelectedTreeChannelsOrderFromCells();
}

function ptzPayload(command: GmvPtzCommand): GbPtzPayload {
  const speed = Math.min(255, Math.max(1, Math.round(command.speed || 1)));
  const zoomSpeed = Math.min(15, speed);
  const payload: GbPtzPayload = { leftRight: 0, upDown: 0, inOut: 0, horizonSpeed: 0, verticalSpeed: 0, zoomSpeed: 0 };
  switch (command.action) {
    case 'left':
      payload.leftRight = 1;
      payload.horizonSpeed = speed;
      break;
    case 'right':
      payload.leftRight = 2;
      payload.horizonSpeed = speed;
      break;
    case 'up':
      payload.upDown = 1;
      payload.verticalSpeed = speed;
      break;
    case 'down':
      payload.upDown = 2;
      payload.verticalSpeed = speed;
      break;
    case 'leftUp':
      payload.leftRight = 1;
      payload.upDown = 1;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'rightUp':
      payload.leftRight = 2;
      payload.upDown = 1;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'leftDown':
      payload.leftRight = 1;
      payload.upDown = 2;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'rightDown':
      payload.leftRight = 2;
      payload.upDown = 2;
      payload.horizonSpeed = speed;
      payload.verticalSpeed = speed;
      break;
    case 'zoomIn':
      payload.inOut = 2;
      payload.zoomSpeed = zoomSpeed;
      break;
    case 'zoomOut':
      payload.inOut = 1;
      payload.zoomSpeed = zoomSpeed;
      break;
  }
  return payload;
}

async function handleMultiPtz(event: { index: number; payload: GmvPtzCommand }) {
  const cell = multiCellAtVisibleIndex(event.index);
  if (!cell) return;
  try {
    await sendGbPtz(cell.device_id, cell.channel_id, ptzPayload(event.payload));
  } catch (error) {
    ElMessage.error(errorMessage(error, '云台控制失败'));
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
        return buildSessionNodeOption(node, undefined, '配置查询失败');
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
    ElMessage.error(errorMessage(error, '设备列表加载失败'));
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
    ElMessage.error(errorMessage(error, '通道列表加载失败'));
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
  if (playerRequesting.value) return;
  const action = kind === 'preview' ? '实时直播' : '历史回放';
  const requestSeq = playRequestSeq + 1;
  playRequestSeq = requestSeq;
  selectedChannel.value = channel;
  lastAction.value = action;
  showImages.value = false;
  playerDialog.value = true;
  playerRequesting.value = true;
  pendingPlayKey.value = playRequestKey(kind, channel);
  try {
    await stopCurrentStream({ closeDialog: false, clearAction: false, cancelPending: false });
    const stream = kind === 'preview'
      ? await startGbPreview(channel.device_id, channel.channel_id, { request_id: 'ui-monitor-preview-' + Date.now(), output_type: 'flv' })
      : await startGbPlayback(channel.device_id, channel.channel_id, { request_id: 'ui-monitor-playback-' + Date.now(), output_type: 'flv' });
    if (requestSeq !== playRequestSeq || !playerDialog.value) {
      if (stream.stream_id) await stopStream(stream.stream_id).catch(() => undefined);
      return;
    }
    lastStream.value = stream;
    ElMessage.success(action + '已提交');
  } catch (error) {
    if (requestSeq === playRequestSeq) ElMessage.error(errorMessage(error, '播放请求失败'));
  } finally {
    if (requestSeq === playRequestSeq) {
      playerRequesting.value = false;
      pendingPlayKey.value = '';
    }
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
    ElMessage.error(errorMessage(error, '抓拍失败'));
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
    ElMessage.error(errorMessage(error, '抓拍图集加载失败'));
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
    ElMessage.error(errorMessage(error, '业务配置保存失败'));
  } finally {
    configSaving.value = false;
  }
}
async function handlePlayerPtz(command: GmvPtzCommand) {
  if (!selectedChannel.value) return;
  try {
    await sendGbPtz(selectedChannel.value.device_id, selectedChannel.value.channel_id, ptzPayload(command));
  } catch (error) {
    ElMessage.error(errorMessage(error, '云台控制失败'));
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

.multi-node-select {
  width: 420px;
  max-width: 100%;
}

.player-controls-toggle {
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--muted);
  font-size: 13px;
  white-space: nowrap;
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
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 4px;
  padding: 12px;
}

.channel-actions .el-button {
  width: 100%;
  margin-left: 0;
}

:deep(.monitor-player-dialog .el-dialog__body) {
  padding: 18px 20px 20px;
}

.monitor-player {
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  gap: 10px;
  min-height: 560px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
  padding: 10px;
}

.monitor-player-toolbar {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  min-height: 34px;
  color: var(--muted);
}

.monitor-player-toolbar span {
  min-width: 0;
  overflow: hidden;
  font-size: 13px;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.monitor-player-stage {
  position: relative;
  min-height: 500px;
  overflow: hidden;
  border-radius: 8px;
}

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

.player-loading-badge {
  position: absolute;
  left: 16px;
  bottom: 16px;
  z-index: 5;
  padding: 7px 10px;
  border: 1px solid rgba(100, 203, 255, .36);
  border-radius: 6px;
  background: rgba(2, 8, 16, .86);
  color: var(--text);
  font-size: 12px;
  letter-spacing: 0;
}

@keyframes waiting-spin {
  to {
    transform: rotate(360deg);
  }
}

@keyframes waiting-scan {
  0% {
    transform: translateY(-100%);
  }

  100% {
    transform: translateY(100%);
  }
}

.monitor-player :deep(.gmv-player) {
  position: relative;
  width: 100%;
  height: 100%;
  min-height: 500px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
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

.monitor-player :deep(.player-topbar) {
  position: absolute;
  inset: 0 0 auto;
  z-index: 2;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 10px;
  padding: 10px 12px;
  background: linear-gradient(180deg, rgba(0, 0, 0, .72), transparent);
}

.monitor-player :deep(.player-topbar strong) {
  display: block;
  color: var(--text);
  font-size: 14px;
}

.monitor-player :deep(.player-topbar span) {
  color: var(--muted);
  font-size: 12px;
}

.monitor-player :deep(.status-strip) {
  display: flex;
  gap: 10px;
  align-items: center;
}

.monitor-player :deep(.status-strip b) {
  color: var(--green);
}

.monitor-player :deep(.reconnect-banner) {
  position: absolute;
  top: 52px;
  left: 12px;
  right: 12px;
  z-index: 3;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 1px solid rgba(244, 180, 0, .45);
  border-radius: 6px;
  background: rgba(52, 33, 9, .82);
}

.monitor-player :deep(.ptz-panel) {
  position: absolute;
  top: 74px;
  right: 12px;
  z-index: 4;
  width: 156px;
  padding: 10px;
  border: 1px solid rgba(100, 203, 255, .22);
  border-radius: 8px;
  background: rgba(3, 10, 24, .86);
  box-shadow: 0 14px 36px rgba(0, 0, 0, .32);
}

.monitor-player :deep(.ptz-grid) {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 5px;
}

.monitor-player :deep(.ptz-grid button) {
  aspect-ratio: 1;
  border-radius: 5px;
}

.monitor-player :deep(.ptz-panel label) {
  display: grid;
  gap: 5px;
  margin: 8px 0;
  color: var(--muted);
  font-size: 12px;
}

.monitor-player :deep(.lens-row) {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 5px;
  margin-top: 5px;
}

.monitor-player :deep(.control-bar) {
  position: absolute;
  inset: auto 0 0;
  z-index: 3;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px;
  background: linear-gradient(0deg, rgba(0, 0, 0, .78), transparent);
}

.monitor-player :deep(.control-bar button),
.monitor-player :deep(.control-bar select),
.monitor-player :deep(.preset-box input) {
  height: 32px;
  border: 1px solid rgba(100, 203, 255, .2);
  border-radius: 5px;
  background: rgba(255, 255, 255, .06);
  color: var(--text);
  padding: 0 9px;
}

.monitor-player :deep(.timeline) {
  display: flex;
  flex: 1;
  align-items: center;
  gap: 8px;
  min-width: 130px;
  color: var(--muted);
}

.monitor-player :deep(.timeline input) {
  width: 100%;
}

.monitor-player :deep(.timeline.disabled) {
  opacity: .45;
}

.monitor-player :deep(.preset-box) {
  display: flex;
  gap: 5px;
}

.tree-device-list,
.tree-workbench {
  display: grid;
  gap: 12px;
}

.tree-device-list {
  height: 352px;
  align-content: start;
  overflow: auto;
}

.tree-channel-label small {
  color: var(--muted);
  font-size: 12px;
}

.tree-channel-label b {
  display: block;
  overflow-wrap: anywhere;
}

.device-channel-tree {
  min-width: 0;
  background: transparent;
  color: var(--text);
}

.device-channel-tree :deep(.el-tree-node__content) {
  min-width: 0;
  height: auto;
  min-height: 42px;
  border-radius: 8px;
  color: var(--text);
}

.device-channel-tree :deep(.el-tree-node__content:hover),
.device-channel-tree :deep(.el-tree-node:focus > .el-tree-node__content) {
  background: rgba(34, 211, 238, .07);
}

.device-channel-tree :deep(.el-tree-node__expand-icon) {
  color: var(--muted);
}

.device-channel-tree :deep(.el-tree-node__expand-icon.expanded) {
  color: var(--cyan);
}

.tree-device-node {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  width: 100%;
  min-width: 0;
  padding: 8px 10px 8px 0;
}

.tree-device-title,
.tree-channel-label {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.tree-device-title b,
.tree-channel-label b,
.tree-channel-label small {
  overflow-wrap: anywhere;
}

.tree-channel-node {
  width: 100%;
  min-width: 0;
  height: auto;
  padding: 7px 10px 7px 0;
  white-space: normal;
}

.tree-channel-node :deep(.el-checkbox__label) {
  min-width: 0;
  white-space: normal;
}

.tree-pagination {
  justify-content: center;
  padding-top: 2px;
}

.selected-channel-panel {
  display: grid;
  grid-template-rows: minmax(0, 1fr);
  min-height: 428px;
}

.selected-channel-list {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
  align-content: start;
  height: 352px;
  overflow: auto;
}

.selected-channel-item {
  display: grid;
  grid-template-columns: minmax(0, 1fr) auto;
  gap: 12px;
  align-items: center;
  padding: 12px;
  border: 1px solid rgba(100, 203, 255, .16);
  border-radius: 8px;
  background: rgba(3, 10, 24, .36);
  cursor: grab;
  user-select: none;
}

.selected-channel-item.dragging {
  opacity: .55;
}

.selected-channel-item b,
.selected-channel-item span {
  display: block;
  overflow-wrap: anywhere;
}

.selected-channel-item span {
  color: var(--muted);
  font-size: 12px;
}

.multi-player {
  display: grid;
  grid-template-rows: minmax(0, 1fr) auto;
  gap: 12px;
  min-height: 620px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
  padding: 12px;
}

.multi-player :deep(.multi-grid) {
  height: 100%;
  min-height: 548px;
}

.multi-pagination {
  display: flex;
  justify-content: center;
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
