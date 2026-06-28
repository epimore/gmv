<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-5" title="GB28181 接入配置" subtitle="先在 GMV 平台创建，再将参数配置到设备端">
      <div class="toolbar">
        <el-input v-model="keyword" style="width: 230px" placeholder="搜索设备 ID / 名称" />
        <el-button :loading="loading" @click="loadDevices">刷新</el-button>
        <el-button type="primary" :disabled="!canOperate" @click="openDevice()">新增接入配置</el-button>
      </div>
      <el-table :data="filteredDevices" height="520" highlight-current-row empty-text="暂无接入配置"
        @current-change="selectDevice">
        <el-table-column prop="device_id" label="SIP设备ID" min-width="190" />
        <el-table-column label="设备名称" min-width="140"><template #default="{ row }">{{ row.alias || '-'
            }}</template></el-table-column>
        <el-table-column prop="domain_id" label="SIP服务器ID" min-width="190" />
        <el-table-column prop="domain" label="SIP域" min-width="110" />
        <el-table-column label="状态" width="90"><template #default="{ row }">
            <StatusPill :label="row.status === 1 ? '启用' : '停用'" :tone="row.status === 1 ? 'ONLINE' : 'OFFLINE'" />
          </template></el-table-column>
        <el-table-column label="密钥" width="80"><template #default="{ row }">{{ row.pwd_check === 1 ? '开启' : '关闭'
            }}</template></el-table-column>
        <el-table-column prop="channel_count" label="通道" width="70" />
        <el-table-column label="操作" width="200" fixed="right">
          <template #default="{ row }">
            <el-button link @click.stop="openDevice(row, true)">查看</el-button>
            <el-button link type="primary" :disabled="!canOperate" @click.stop="openDevice(row)">编辑</el-button>
            <el-button link type="danger" :disabled="!canOperate" @click.stop="removeDevice(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-7" title="通道列表" :subtitle="selectedDevice ? selectedDevice.device_id : '选择设备后查看通道'">
      <div class="toolbar">
        <el-button :disabled="!selectedDevice" @click="loadChannels">刷新通道</el-button>
        <el-button type="primary" :disabled="!selectedDevice || !canOperate" @click="openChannel()">新增通道</el-button>
      </div>
      <el-table :data="channels" height="520" highlight-current-row empty-text="暂无通道"
        @current-change="selectedChannel = $event">
        <el-table-column prop="channel_id" label="通道 ID" min-width="190" />
        <el-table-column prop="name" label="名称" min-width="160" />
        <el-table-column label="状态" width="110"><template #default="{ row }">
            <StatusPill :label="row.status || 'UNKNOWN'" :tone="statusTone(row.status)" />
          </template></el-table-column>
        <el-table-column prop="address" label="位置" min-width="150" />
        <el-table-column label="能力" width="150">
          <template #default="{ row }"><span class="cap">{{ row.ptz_enable ? 'PTZ' : '-' }}</span><span class="cap">{{
            row.playback_enable ? '回放' : '-' }}</span></template>
        </el-table-column>
        <el-table-column label="操作" width="270" fixed="right">
          <template #default="{ row }">
            <el-button link type="primary" :disabled="!canOperate" @click.stop="operate(row, 'preview')">点播</el-button>
            <el-button link :disabled="!canOperate" @click.stop="operate(row, 'playback')">回放</el-button>
            <el-button link :disabled="!canOperate" @click.stop="operate(row, 'ptz')">云台</el-button>
            <el-button link :disabled="!canOperate" @click.stop="operate(row, 'snapshot')">拍照</el-button>
            <el-dropdown trigger="click"
              @command="(command: string) => command === 'edit' ? openChannel(row) : removeChannel(row)">
              <el-button link>更多</el-button>
              <template #dropdown><el-dropdown-menu><el-dropdown-item
                    command="edit">编辑</el-dropdown-item><el-dropdown-item command="delete"
                    :disabled="!canOperate">删除</el-dropdown-item></el-dropdown-menu></template>
            </el-dropdown>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-12" title="实时播放" :subtitle="playerSubtitle">
      <div v-if="playerSources.length" class="player-shell">
        <GmvPlayerView :sources="playerSources" :device-id="selectedChannel?.device_id"
          :channel-id="selectedChannel?.channel_id" :title="playerTitle" :status="playerStatus" :viewers="1"
          :osd="playerOsd" :capabilities="playerCapabilities" @snapshot="handlePlayerSnapshot" @ptz="handlePlayerPtz" />
      </div>
      <el-empty v-else description="选择通道后点击点播" />
    </GlassPanel>

    <GlassPanel class="span-12" title="设备端配置参数" subtitle="保存后，将这些参数配置到 GB28181 设备端">
      <div class="result-grid">
        <div class="kv-item"><span>SIP设备ID</span><b>{{ selectedDevice?.device_id ?? '-' }}</b></div>
        <div class="kv-item"><span>SIP服务器ID</span><b>{{ selectedDevice?.domain_id ?? '-' }}</b></div>
        <div class="kv-item"><span>SIP域</span><b>{{ selectedDevice?.domain ?? '-' }}</b></div>
        <div class="kv-item"><span>密码</span><b>{{ selectedDevice?.pwd ?? '-' }}</b></div>
      </div>
      <div class="result-grid">
        <div class="kv-item"><span>心跳周期</span><b>{{ selectedDevice?.heartbeat_sec ?? '-' }} 秒</b></div>
        <div class="kv-item"><span>Session节点</span><b>{{ selectedDevice?.session_node_id ?? '-' }}</b></div>
        <div class="kv-item"><span>最近操作</span><b>{{ lastAction || '-' }}</b></div>
        <div class="kv-item"><span>返回端点</span><b>{{ lastStream?.endpoint ?? '-' }}</b></div>
      </div>
      <el-table :data="images" height="180" empty-text="暂无抓图记录">
        <el-table-column prop="image_id" label="图片 ID" min-width="220" />
        <el-table-column prop="image_url" label="地址" min-width="360" />
        <el-table-column prop="created_at_ms" label="时间" width="180" />
      </el-table>
    </GlassPanel>

    <el-dialog v-model="deviceDialog" :title="deviceDialogTitle" width="820px">
      <el-form :model="deviceForm" label-width="130px">
        <el-form-item label="SIP设备ID"><el-input v-model="deviceForm.device_id" :disabled="deviceReadonly || !!editingDevice"
            placeholder="新增时留空，由平台按 SIP 服务器 ID 前缀递增生成" /></el-form-item>
        <el-form-item label="Session 节点" required>
          <el-select v-model="deviceForm.session_node_id" filterable placeholder="请选择 session 节点" style="width: 100%" :disabled="deviceReadonly"
            @change="selectSessionNode">
            <el-option v-for="node in sessionNodes" :key="node.node_id"
              :label="node.display_name + ' · ' + node.node_id" :value="node.node_id" />
          </el-select>
        </el-form-item>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="SIP服务器ID" required><div class="derived-value">{{ deviceForm.domain_id || "-" }}</div></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="SIP域" required><div class="derived-value">{{ deviceForm.domain || "-" }}</div></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="设备别名"><el-input v-model="deviceForm.alias" :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="状态"><el-switch v-model="deviceForm.status" :active-value="1"
                :inactive-value="0" active-text="启用" inactive-text="停用" :disabled="deviceReadonly" /></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="密钥认证"><el-switch v-model="deviceForm.pwd_check" :active-value="1"
                :inactive-value="0" active-text="开启" inactive-text="关闭" :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="密钥"><el-input v-model="deviceForm.pwd"
                :disabled="deviceReadonly || deviceForm.pwd_check !== 1" /></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="心跳周期(秒)"><el-input-number v-model="deviceForm.heartbeat_sec" :min="5"
                :max="255" style="width: 100%" :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="地址"><el-input v-model="deviceForm.address" :disabled="deviceReadonly" /></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="经度"><el-input
                v-model="deviceForm.longitude" :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="纬度"><el-input v-model="deviceForm.latitude" :disabled="deviceReadonly" /></el-form-item></el-col>
        </el-row>
        <!-- <el-row :gutter="16">
          <el-col :span="8"><el-form-item label="tenant_id"><el-input
                v-model="deviceForm.tenant_id" /></el-form-item></el-col>
          <el-col :span="8"><el-form-item label="sys_org_code"><el-input
                v-model="deviceForm.sys_org_code" /></el-form-item></el-col>
          <el-col :span="8"><el-form-item label="create_by"><el-input
                v-model="deviceForm.create_by" /></el-form-item></el-col>
        </el-row> -->
      </el-form>
      <template #footer><el-button @click="deviceDialog = false">取消</el-button><el-button v-if="!deviceReadonly" type="primary"
          :disabled="!canOperate" @click="saveDevice">保存</el-button></template>
    </el-dialog>

    <el-dialog v-model="channelDialog" :title="editingChannel ? '编辑通道' : '新增通道'" width="760px">
      <el-form :model="channelForm" label-width="110px">
        <el-form-item label="通道 ID"><el-input v-model="channelForm.channel_id"
            :disabled="!!editingChannel" /></el-form-item>
        <el-form-item label="名称"><el-input v-model="channelForm.name" /></el-form-item>
        <el-form-item label="状态"><el-input v-model="channelForm.status"
            placeholder="ONLINE/OFFLINE/UNKNOWN" /></el-form-item>
        <el-form-item label="地址"><el-input v-model="channelForm.address" /></el-form-item>
        <el-form-item label="经纬度"><el-input v-model="channelForm.longitude" placeholder="经度" /><el-input
            v-model="channelForm.latitude" placeholder="纬度" style="margin-top: 8px" /></el-form-item>
        <el-form-item label="能力"><el-checkbox v-model="channelFlags.ptz">PTZ</el-checkbox><el-checkbox
            v-model="channelFlags.playback">回放</el-checkbox><el-checkbox
            v-model="channelFlags.record">录像</el-checkbox></el-form-item>
      </el-form>
      <template #footer><el-button @click="channelDialog = false">取消</el-button><el-button type="primary"
          :disabled="!canOperate" @click="saveChannel">保存</el-button></template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { createGbChannel, createGbDevice, deleteGbChannel, deleteGbDevice, listGbChannelImages, listGbChannels, listGbDevices, listNodes, sendGbPtz, startGbPlayback, startGbPreview, takeGbSnapshot, updateGbChannel, updateGbDevice, type GbChannelImageInfo, type GbChannelInfo, type GbChannelPayload, type GbDeviceInfo, type GbDevicePayload, type NodeInfo, type StreamSummary } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { GmvPlayerView, type GmvPtzCommand, type GmvSource } from 'gmv-player';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const loading = ref(false);
const keyword = ref('');
const devices = ref<GbDeviceInfo[]>([]);
const channels = ref<GbChannelInfo[]>([]);
const images = ref<GbChannelImageInfo[]>([]);
const sessionNodes = ref<NodeInfo[]>([]);
const selectedDevice = ref<GbDeviceInfo>();
const selectedChannel = ref<GbChannelInfo>();
const lastAction = ref('');
const lastStream = ref<StreamSummary>();
const deviceDialog = ref(false);
const deviceReadonly = ref(false);
const channelDialog = ref(false);
const editingDevice = ref<GbDeviceInfo>();
const editingChannel = ref<GbChannelInfo>();
const deviceForm = reactive<GbDevicePayload>(emptyDevice());
const channelForm = reactive<GbChannelPayload>(emptyChannel());
const channelFlags = reactive({ ptz: false, playback: false, record: false });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const deviceDialogTitle = computed(() => deviceReadonly.value ? "查看接入配置" : editingDevice.value ? "编辑接入配置" : "新增接入配置");
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
const playerTitle = computed(() => selectedChannel.value?.name || selectedChannel.value?.channel_id || "实时播放");
const playerSubtitle = computed(() => lastStream.value?.endpoint || "选择通道后点击点播");
const playerStatus = computed(() => lastStream.value?.state === "running" ? "playing" : (selectedChannel.value?.status || "").toLowerCase().includes("online") ? "online" : "idle");
const playerOsd = computed(() => [
  { id: "channel", text: selectedChannel.value?.name || selectedChannel.value?.channel_id || "未选择通道", x: 3, y: 5 },
  { id: "mode", text: lastAction.value || "preview", x: 3, y: 12 },
]);
const playerSources = computed<GmvSource[]>(() => {
  const endpoint = lastStream.value?.endpoint;
  if (!endpoint) return [];
  const protocol = streamProtocol(endpoint);
  return [{
    protocol,
    codec: "h265",
    url: endpoint,
    mimeCodec: protocol === "fmp4" ? "video/mp4; codecs=\"hvc1.1.6.L123.B0, mp4a.40.2\"" : undefined,
    hasAudio: false,
    label: "默认静音",
    priority: 1,
  }];
});
const filteredDevices = computed(() => devices.value.filter((item) => !keyword.value || item.device_id.includes(keyword.value) || (item.alias || '').includes(keyword.value)));

function emptyDevice(): GbDevicePayload { return { device_id: '', alias: '', session_node_id: '', domain_id: '', domain: '', pwd_check: 1, pwd: '', status: 1, heartbeat_sec: 60, address: '', longitude: '', latitude: '', tenant_id: '', sys_org_code: '', create_by: '' }; }
function emptyChannel(): GbChannelPayload { return { channel_id: '', name: '', status: 'UNKNOWN', address: '', longitude: '', latitude: '', ptz_enable: 0, playback_enable: 0, record_enable: 0, talk_enable: 0, audio_enable: 0, alarm_enable: 0, biz_enable: 0, snapshot: 0, sort_no: 0 }; }
function assign<T extends object>(target: T, source: Partial<T>) { Object.assign(target, source); }
function statusTone(status?: string) { return (status || '').toLowerCase().includes('online') ? 'ONLINE' : (status || 'UNKNOWN'); }
async function loadSessionNodes() { const nodes = await listNodes(); sessionNodes.value = nodes.filter((node) => node.service === 'session' || node.protocol === 'gb28181' || node.capabilities.some((item) => item.includes('device.'))); }
async function loadDevices() { loading.value = true; try { await loadSessionNodes().catch(() => undefined); devices.value = await listGbDevices(); if (!selectedDevice.value && devices.value[0]) await selectDevice(devices.value[0]); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '设备加载失败'); } finally { loading.value = false; } }
async function selectDevice(row?: GbDeviceInfo) { selectedDevice.value = row; selectedChannel.value = undefined; images.value = []; await loadChannels(); }
function streamProtocol(endpoint: string): GmvSource["protocol"] {
  const path = endpoint.split("?")[0].toLowerCase();
  if (path.endsWith(".fmp4")) return "fmp4";
  if (path.endsWith(".m3u8")) return "hls";
  return "flv";
}
async function handlePlayerSnapshot() {
  if (!selectedChannel.value) return;
  try {
    await takeGbSnapshot(selectedChannel.value.device_id, selectedChannel.value.channel_id);
    await loadImages(selectedChannel.value);
    ElMessage.success("抓拍已提交");
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : "抓拍失败");
  }
}
async function handlePlayerPtz(command: GmvPtzCommand) {
  if (!selectedChannel.value || command.action === "stop") return;
  try {
    await sendGbPtz(selectedChannel.value.device_id, selectedChannel.value.channel_id);
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : "云台控制失败");
  }
}
async function loadChannels() { if (!selectedDevice.value) { channels.value = []; return; } try { channels.value = await listGbChannels(selectedDevice.value.device_id); selectedChannel.value = channels.value[0]; if (selectedChannel.value) await loadImages(selectedChannel.value); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '通道加载失败'); } }
async function loadImages(row: GbChannelInfo) { images.value = await listGbChannelImages(row.device_id, row.channel_id).catch(() => []); }
function applySessionNodeConfig(nodeId: string) { const node = sessionNodes.value.find((item) => item.node_id === nodeId); if (!node) return false; deviceForm.domain_id = node.config.domain_id || ""; deviceForm.domain = node.config.domain || ""; return true; }
function selectSessionNode(nodeId: string) { applySessionNodeConfig(nodeId); }
function openDevice(row?: GbDeviceInfo, readonly = false) { deviceReadonly.value = readonly; editingDevice.value = row; const payload: GbDevicePayload = row ? { device_id: row.device_id, session_node_id: row.session_node_id, domain_id: row.domain_id, domain: row.domain, longitude: row.longitude || "", latitude: row.latitude || "", address: row.address || "", pwd: row.pwd || "", pwd_check: row.pwd_check, alias: row.alias || "", status: row.status, heartbeat_sec: row.heartbeat_sec, tenant_id: row.tenant_id || "", sys_org_code: row.sys_org_code || "", create_by: row.create_by || "", update_by: row.update_by || "" } : emptyDevice(); assign(deviceForm, payload); if (deviceForm.session_node_id) applySessionNodeConfig(deviceForm.session_node_id); deviceDialog.value = true; }
async function saveDevice() { const nodeId = deviceForm.session_node_id; if (!nodeId) return ElMessage.warning("Session 节点必填"); const node = sessionNodes.value.find((item) => item.node_id === nodeId); const domain_id = node?.config.domain_id || ""; const domain = node?.config.domain || ""; if (!domain_id || !domain) return ElMessage.warning("所选 Session 节点缺少 domain_id/domain 配置"); const payload = { ...deviceForm, domain_id, domain }; try { const saved = editingDevice.value ? await updateGbDevice(editingDevice.value.device_id, payload) : await createGbDevice(payload); deviceDialog.value = false; await loadDevices(); selectedDevice.value = devices.value.find((item) => item.device_id === saved.device_id); ElMessage.success("接入配置已保存"); } catch (error) { ElMessage.error(error instanceof Error ? error.message : "设备保存失败"); } }
async function removeDevice(row: GbDeviceInfo) { await ElMessageBox.confirm(`确认删除接入配置 ${row.device_id}？`, '删除确认', { type: 'warning' }); await deleteGbDevice(row.device_id); if (selectedDevice.value?.device_id === row.device_id) { selectedDevice.value = undefined; channels.value = []; } await loadDevices(); ElMessage.success('接入配置已删除'); }
function openChannel(row?: GbChannelInfo) { if (!selectedDevice.value) return; editingChannel.value = row; assign(channelForm, row ? { ...row } : emptyChannel()); channelFlags.ptz = Number(row?.ptz_enable ?? 0) > 0; channelFlags.playback = Number(row?.playback_enable ?? 0) > 0; channelFlags.record = Number(row?.record_enable ?? 0) > 0; channelDialog.value = true; }
async function saveChannel() { if (!selectedDevice.value || !channelForm.channel_id) return ElMessage.warning('通道 ID 必填'); const payload: GbChannelPayload = { ...channelForm, ptz_enable: channelFlags.ptz ? 1 : 0, playback_enable: channelFlags.playback ? 1 : 0, record_enable: channelFlags.record ? 1 : 0 }; try { const saved = editingChannel.value ? await updateGbChannel(selectedDevice.value.device_id, editingChannel.value.channel_id, payload) : await createGbChannel(selectedDevice.value.device_id, payload); channelDialog.value = false; await loadChannels(); selectedChannel.value = channels.value.find((item) => item.channel_id === saved.channel_id); ElMessage.success('通道已保存'); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '通道保存失败'); } }
async function removeChannel(row: GbChannelInfo) { await ElMessageBox.confirm(`确认删除通道 ${row.channel_id}？`, '删除确认', { type: 'warning' }); await deleteGbChannel(row.device_id, row.channel_id); await loadChannels(); ElMessage.success('通道已删除'); }
async function operate(row: GbChannelInfo, action: 'preview' | 'playback' | 'ptz' | 'snapshot') { selectedChannel.value = row; try { if (action === 'preview') lastStream.value = await startGbPreview(row.device_id, row.channel_id, { request_id: 'ui-preview-' + Date.now(), output_type: 'flv' }); if (action === 'playback') lastStream.value = await startGbPlayback(row.device_id, row.channel_id, { request_id: 'ui-playback-' + Date.now(), output_type: 'flv' }); if (action === 'ptz') await sendGbPtz(row.device_id, row.channel_id); if (action === 'snapshot') await takeGbSnapshot(row.device_id, row.channel_id); lastAction.value = action; await loadImages(row); ElMessage.success('操作已提交'); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '操作失败'); } }
onMounted(loadDevices);
</script>

<style scoped>
.cap {
  display: inline-flex;
  margin-right: 6px;
  color: var(--cyan);
  font-size: 12px;
}

.derived-value {
  min-height: 32px;
  display: flex;
  align-items: center;
  padding: 0 10px;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 6px;
  background: rgba(255, 255, 255, .04);
  color: var(--text);
  word-break: break-all;
}

.result-grid {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 12px;
  margin-bottom: 14px;
}

.player-shell {
  height: 420px;
  min-height: 420px;
  overflow: hidden;
  border: 1px solid rgba(100, 203, 255, .18);
  border-radius: 8px;
  background: #02050a;
}

.player-shell :deep(.gmv-player) {
  position: relative;
  width: 100%;
  height: 100%;
  min-height: 420px;
  overflow: hidden;
  background: #02050a;
  color: var(--text);
}

.player-shell :deep(.gmv-video) {
  width: 100%;
  height: 100%;
  display: block;
  object-fit: contain;
  background: #02050a;
}

.player-shell :deep(.gmv-layer) {
  position: absolute;
  inset: 0;
  pointer-events: none;
}

.player-shell :deep(.osd-item) {
  position: absolute;
  padding: 2px 7px;
  border-radius: 4px;
  background: rgba(0, 0, 0, .48);
  font-size: 12px;
  color: #fff;
}

.player-shell :deep(.player-topbar) {
  position: absolute;
  top: 0;
  left: 0;
  right: 0;
  display: flex;
  justify-content: space-between;
  gap: 10px;
  padding: 9px 10px;
  background: linear-gradient(180deg, rgba(0, 0, 0, .68), transparent);
}

.player-shell :deep(.player-topbar strong) {
  display: block;
  font-size: 14px;
}

.player-shell :deep(.player-topbar span) {
  color: var(--muted);
  font-size: 12px;
}

.player-shell :deep(.status-strip) {
  display: flex;
  gap: 10px;
  align-items: center;
}

.player-shell :deep(.status-strip b) {
  color: var(--green);
}

.player-shell :deep(.reconnect-banner) {
  position: absolute;
  top: 48px;
  left: 10px;
  right: 10px;
  display: flex;
  justify-content: space-between;
  align-items: center;
  gap: 10px;
  padding: 8px 10px;
  border: 1px solid rgba(251, 191, 36, .45);
  background: rgba(52, 33, 9, .82);
  border-radius: 6px;
}

.player-shell :deep(.ptz-panel) {
  position: absolute;
  right: 10px;
  top: 74px;
  width: 150px;
  padding: 9px;
  border-radius: 8px;
  border: 1px solid rgba(110, 201, 255, .2);
  background: rgba(10, 17, 26, .72);
}

.player-shell :deep(.ptz-grid) {
  display: grid;
  grid-template-columns: repeat(3, 1fr);
  gap: 5px;
}

.player-shell :deep(.ptz-grid button) {
  aspect-ratio: 1;
  border-radius: 5px;
}

.player-shell :deep(.ptz-panel label) {
  display: grid;
  gap: 4px;
  margin: 8px 0;
  color: var(--muted);
  font-size: 12px;
}

.player-shell :deep(.lens-row) {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 5px;
  margin-top: 5px;
}

.player-shell :deep(.lens-row button) {
  height: 28px;
  border-radius: 5px;
  font-size: 12px;
}

.player-shell :deep(.control-bar) {
  position: absolute;
  left: 0;
  right: 0;
  bottom: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px;
  background: linear-gradient(0deg, rgba(0, 0, 0, .74), transparent);
}

.player-shell :deep(.control-bar button),
.player-shell :deep(.control-bar select),
.player-shell :deep(.preset-box input) {
  height: 32px;
  border-radius: 5px;
  padding: 0 9px;
}

.player-shell :deep(.timeline) {
  flex: 1;
  min-width: 130px;
  display: flex;
  align-items: center;
  gap: 8px;
  color: var(--muted);
}

.player-shell :deep(.timeline input) {
  width: 100%;
}
</style>
