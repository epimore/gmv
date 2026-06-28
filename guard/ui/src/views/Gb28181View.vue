<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-5" title="GB28181 设备台账" subtitle="设备基础信息维护与节点归属">
      <div class="toolbar">
        <el-input v-model="keyword" style="width: 230px" placeholder="搜索设备 ID / 名称" />
        <el-button :loading="loading" @click="loadDevices">刷新</el-button>
        <el-button type="primary" :disabled="!canOperate" @click="openDevice()">新增设备</el-button>
      </div>
      <el-table :data="filteredDevices" height="520" highlight-current-row empty-text="暂无设备" @current-change="selectDevice">
        <el-table-column prop="device_id" label="设备 ID" min-width="180" />
        <el-table-column prop="alias" label="名称" min-width="150" />
        <el-table-column label="状态" width="110"><template #default="{ row }"><StatusPill :label="row.status || 'UNKNOWN'" :tone="statusTone(row.status)" /></template></el-table-column>
        <el-table-column prop="channel_count" label="通道" width="80" />
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button link type="primary" @click.stop="openDevice(row)">编辑</el-button>
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
      <el-table :data="channels" height="520" highlight-current-row empty-text="暂无通道" @current-change="selectedChannel = $event">
        <el-table-column prop="channel_id" label="通道 ID" min-width="190" />
        <el-table-column prop="name" label="名称" min-width="160" />
        <el-table-column label="状态" width="110"><template #default="{ row }"><StatusPill :label="row.status || 'UNKNOWN'" :tone="statusTone(row.status)" /></template></el-table-column>
        <el-table-column prop="address" label="位置" min-width="150" />
        <el-table-column label="能力" width="150">
          <template #default="{ row }">
            <span class="cap">{{ row.ptz_enable ? 'PTZ' : '-' }}</span>
            <span class="cap">{{ row.playback_enable ? '回放' : '-' }}</span>
          </template>
        </el-table-column>
        <el-table-column label="操作" width="270" fixed="right">
          <template #default="{ row }">
            <el-button link type="primary" :disabled="!canOperate" @click.stop="operate(row, 'preview')">点播</el-button>
            <el-button link :disabled="!canOperate" @click.stop="operate(row, 'playback')">回放</el-button>
            <el-button link :disabled="!canOperate" @click.stop="operate(row, 'ptz')">云台</el-button>
            <el-button link :disabled="!canOperate" @click.stop="operate(row, 'snapshot')">拍照</el-button>
            <el-dropdown trigger="click" @command="(command: string) => command === 'edit' ? openChannel(row) : removeChannel(row)">
              <el-button link>更多</el-button>
              <template #dropdown><el-dropdown-menu><el-dropdown-item command="edit">编辑</el-dropdown-item><el-dropdown-item command="delete" :disabled="!canOperate">删除</el-dropdown-item></el-dropdown-menu></template>
            </el-dropdown>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-12" title="操作回执" subtitle="所有控制命令均通过 Guard 后端接口发起">
      <div class="result-grid">
        <div class="kv-item"><span>当前设备</span><b>{{ selectedDevice?.device_id ?? '-' }}</b></div>
        <div class="kv-item"><span>当前通道</span><b>{{ selectedChannel?.channel_id ?? '-' }}</b></div>
        <div class="kv-item"><span>最近操作</span><b>{{ lastAction || '-' }}</b></div>
        <div class="kv-item"><span>返回端点</span><b>{{ lastStream?.endpoint ?? '-' }}</b></div>
      </div>
      <el-table :data="images" height="180" empty-text="暂无抓图记录">
        <el-table-column prop="image_id" label="图片 ID" min-width="220" />
        <el-table-column prop="image_url" label="地址" min-width="360" />
        <el-table-column prop="created_at_ms" label="时间" width="180" />
      </el-table>
    </GlassPanel>

    <el-dialog v-model="deviceDialog" :title="editingDevice ? '编辑设备' : '新增设备'" width="720px">
      <el-form :model="deviceForm" label-width="110px">
        <el-form-item label="设备 ID"><el-input v-model="deviceForm.device_id" :disabled="!!editingDevice" /></el-form-item>
        <el-form-item label="名称"><el-input v-model="deviceForm.alias" /></el-form-item>
        <el-form-item label="Session 节点"><el-input v-model="deviceForm.session_node_id" /></el-form-item>
        <el-form-item label="传输"><el-input v-model="deviceForm.transport" placeholder="UDP/TCP" /></el-form-item>
        <el-form-item label="厂商"><el-input v-model="deviceForm.manufacturer" /></el-form-item>
        <el-form-item label="型号"><el-input v-model="deviceForm.model" /></el-form-item>
        <el-form-item label="版本"><el-input v-model="deviceForm.gb_version" /></el-form-item>
        <el-form-item label="状态"><el-input v-model="deviceForm.status" placeholder="ONLINE/OFFLINE/UNKNOWN" /></el-form-item>
      </el-form>
      <template #footer><el-button @click="deviceDialog = false">取消</el-button><el-button type="primary" :disabled="!canOperate" @click="saveDevice">保存</el-button></template>
    </el-dialog>

    <el-dialog v-model="channelDialog" :title="editingChannel ? '编辑通道' : '新增通道'" width="760px">
      <el-form :model="channelForm" label-width="110px">
        <el-form-item label="通道 ID"><el-input v-model="channelForm.channel_id" :disabled="!!editingChannel" /></el-form-item>
        <el-form-item label="名称"><el-input v-model="channelForm.name" /></el-form-item>
        <el-form-item label="状态"><el-input v-model="channelForm.status" placeholder="ONLINE/OFFLINE/UNKNOWN" /></el-form-item>
        <el-form-item label="地址"><el-input v-model="channelForm.address" /></el-form-item>
        <el-form-item label="经纬度"><el-input v-model="channelForm.longitude" placeholder="经度" /><el-input v-model="channelForm.latitude" placeholder="纬度" style="margin-top: 8px" /></el-form-item>
        <el-form-item label="能力"><el-checkbox v-model="channelFlags.ptz">PTZ</el-checkbox><el-checkbox v-model="channelFlags.playback">回放</el-checkbox><el-checkbox v-model="channelFlags.record">录像</el-checkbox></el-form-item>
      </el-form>
      <template #footer><el-button @click="channelDialog = false">取消</el-button><el-button type="primary" :disabled="!canOperate" @click="saveChannel">保存</el-button></template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { createGbChannel, createGbDevice, deleteGbChannel, deleteGbDevice, listGbChannelImages, listGbChannels, listGbDevices, sendGbPtz, startGbPlayback, startGbPreview, takeGbSnapshot, updateGbChannel, updateGbDevice, type GbChannelImageInfo, type GbChannelInfo, type GbChannelPayload, type GbDeviceInfo, type GbDevicePayload, type StreamSummary } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const loading = ref(false);
const keyword = ref('');
const devices = ref<GbDeviceInfo[]>([]);
const channels = ref<GbChannelInfo[]>([]);
const images = ref<GbChannelImageInfo[]>([]);
const selectedDevice = ref<GbDeviceInfo>();
const selectedChannel = ref<GbChannelInfo>();
const lastAction = ref('');
const lastStream = ref<StreamSummary>();
const deviceDialog = ref(false);
const channelDialog = ref(false);
const editingDevice = ref<GbDeviceInfo>();
const editingChannel = ref<GbChannelInfo>();
const deviceForm = reactive<GbDevicePayload>(emptyDevice());
const channelForm = reactive<GbChannelPayload>(emptyChannel());
const channelFlags = reactive({ ptz: false, playback: false, record: false });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const filteredDevices = computed(() => devices.value.filter((item) => !keyword.value || item.device_id.includes(keyword.value) || item.alias.includes(keyword.value)));

function emptyDevice(): GbDevicePayload { return { device_id: '', alias: '', session_node_id: '', transport: 'UDP', device_type: 'GB28181', manufacturer: '', model: '', gb_version: 'GB/T 28181-2016', status: 'UNKNOWN', camera_in_count: 0, camera_off_count: 0 }; }
function emptyChannel(): GbChannelPayload { return { channel_id: '', name: '', status: 'UNKNOWN', address: '', longitude: '', latitude: '', ptz_enable: 0, playback_enable: 0, record_enable: 0, talk_enable: 0, audio_enable: 0, alarm_enable: 0, biz_enable: 0, snapshot: 0, sort_no: 0 }; }
function assign<T extends object>(target: T, source: T) { Object.assign(target, source); }
function statusTone(status?: string) { return (status || '').toLowerCase().includes('online') ? 'ONLINE' : (status || 'UNKNOWN'); }
async function loadDevices() { loading.value = true; try { devices.value = await listGbDevices(); if (!selectedDevice.value && devices.value[0]) await selectDevice(devices.value[0]); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '设备加载失败'); } finally { loading.value = false; } }
async function selectDevice(row?: GbDeviceInfo) { selectedDevice.value = row; selectedChannel.value = undefined; images.value = []; await loadChannels(); }
async function loadChannels() { if (!selectedDevice.value) { channels.value = []; return; } try { channels.value = await listGbChannels(selectedDevice.value.device_id); selectedChannel.value = channels.value[0]; if (selectedChannel.value) await loadImages(selectedChannel.value); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '通道加载失败'); } }
async function loadImages(row: GbChannelInfo) { images.value = await listGbChannelImages(row.device_id, row.channel_id).catch(() => []); }
function openDevice(row?: GbDeviceInfo) { editingDevice.value = row; assign(deviceForm, row ? { ...row } : emptyDevice()); deviceDialog.value = true; }
async function saveDevice() { const payload = { ...deviceForm }; if (!payload.device_id) return ElMessage.warning('设备 ID 必填'); try { const saved = editingDevice.value ? await updateGbDevice(editingDevice.value.device_id, payload) : await createGbDevice(payload); deviceDialog.value = false; await loadDevices(); selectedDevice.value = devices.value.find((item) => item.device_id === saved.device_id); ElMessage.success('设备已保存'); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '设备保存失败'); } }
async function removeDevice(row: GbDeviceInfo) { await ElMessageBox.confirm(`确认删除设备 ${row.device_id}？`, '删除确认', { type: 'warning' }); await deleteGbDevice(row.device_id); if (selectedDevice.value?.device_id === row.device_id) { selectedDevice.value = undefined; channels.value = []; } await loadDevices(); ElMessage.success('设备已删除'); }
function openChannel(row?: GbChannelInfo) { if (!selectedDevice.value) return; editingChannel.value = row; assign(channelForm, row ? { ...row } : emptyChannel()); channelFlags.ptz = Number(row?.ptz_enable ?? 0) > 0; channelFlags.playback = Number(row?.playback_enable ?? 0) > 0; channelFlags.record = Number(row?.record_enable ?? 0) > 0; channelDialog.value = true; }
async function saveChannel() { if (!selectedDevice.value || !channelForm.channel_id) return ElMessage.warning('通道 ID 必填'); const payload: GbChannelPayload = { ...channelForm, ptz_enable: channelFlags.ptz ? 1 : 0, playback_enable: channelFlags.playback ? 1 : 0, record_enable: channelFlags.record ? 1 : 0 }; try { const saved = editingChannel.value ? await updateGbChannel(selectedDevice.value.device_id, editingChannel.value.channel_id, payload) : await createGbChannel(selectedDevice.value.device_id, payload); channelDialog.value = false; await loadChannels(); selectedChannel.value = channels.value.find((item) => item.channel_id === saved.channel_id); ElMessage.success('通道已保存'); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '通道保存失败'); } }
async function removeChannel(row: GbChannelInfo) { await ElMessageBox.confirm(`确认删除通道 ${row.channel_id}？`, '删除确认', { type: 'warning' }); await deleteGbChannel(row.device_id, row.channel_id); await loadChannels(); ElMessage.success('通道已删除'); }
async function operate(row: GbChannelInfo, action: 'preview' | 'playback' | 'ptz' | 'snapshot') { selectedChannel.value = row; try { if (action === 'preview') lastStream.value = await startGbPreview(row.device_id, row.channel_id, { request_id: 'ui-preview-' + Date.now(), output_type: 'flv' }); if (action === 'playback') lastStream.value = await startGbPlayback(row.device_id, row.channel_id, { request_id: 'ui-playback-' + Date.now(), output_type: 'hls' }); if (action === 'ptz') await sendGbPtz(row.device_id, row.channel_id); if (action === 'snapshot') await takeGbSnapshot(row.device_id, row.channel_id); lastAction.value = action; await loadImages(row); ElMessage.success('操作已提交'); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '操作失败'); } }
onMounted(loadDevices);
</script>

<style scoped>
.cap { display: inline-flex; margin-right: 6px; color: var(--cyan); font-size: 12px; }
.result-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 12px; margin-bottom: 14px; }
</style>
