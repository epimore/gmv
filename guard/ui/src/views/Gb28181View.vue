<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-12">
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
        <el-input v-model="deviceId" style="width: 230px" placeholder="设备 ID" clearable />
        <el-input v-model="deviceName" style="width: 180px" placeholder="设备名称" clearable />
        <el-button type="primary" :loading="loading" @click="queryDevices">查询</el-button>
        <el-button :loading="loading" @click="resetDevices">重置</el-button>
        <el-button type="primary" :disabled="!canOperate" @click="openDevice()">新增注册配置</el-button>
      </div>
      <el-table :data="devices" height="520" empty-text="暂无注册配置">
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
        <el-table-column label="心跳周期" width="100"><template #default="{ row }">{{ row.heartbeat_sec }}
            秒</template></el-table-column>
        <el-table-column label="创建时间" min-width="170" show-overflow-tooltip>
          <template #default="{ row }">{{ row.create_time || '-' }}</template>
        </el-table-column>
        <el-table-column label="操作" width="200" fixed="right">
          <template #default="{ row }">
            <el-button link @click.stop="openDevice(row, true)">查看</el-button>
            <el-button link type="primary" :disabled="!canOperate" @click.stop="openDevice(row)">编辑</el-button>
            <el-button link type="danger" :disabled="!canOperate" @click.stop="removeDevice(row)">删除</el-button>
          </template>
        </el-table-column>
      </el-table>
      <div class="pagination-bar">
        <el-pagination v-model:current-page="page" v-model:page-size="pageSize" :total="total"
          :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next, jumper" @current-change="loadDevices"
          @size-change="handlePageSizeChange" />
      </div>
    </GlassPanel>

    <el-dialog v-model="deviceDialog" :title="deviceDialogTitle" width="820px">
      <el-form :model="deviceForm" label-width="130px">
        <el-form-item label="SIP设备ID"><el-input v-model="deviceForm.device_id"
            :disabled="deviceReadonly || !!editingDevice" placeholder="新增时留空，由平台按 SIP 服务器 ID 前缀递增生成" /></el-form-item>
        <el-form-item label="Session 节点" required>
          <el-select v-model="deviceForm.session_node_id" filterable placeholder="请选择 session 节点" style="width: 100%"
            :disabled="deviceReadonly" :loading="sessionConfigLoading" @change="selectSessionNode">
            <el-option v-for="node in sessionNodes" :key="node.node_id" :label="nodeLabel(node)" :value="node.node_id"
              :disabled="!isNodeOnline(node)">
              <div class="node-option" :class="{ offline: !isNodeOnline(node) }">
                <span>{{ nodeKindLabel(node) }} · {{ node.node_id }}</span>
                <span class="node-status">{{ isNodeOnline(node) ? "在线" : "离线" }}</span>
              </div>
            </el-option>
          </el-select>
        </el-form-item>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="SIP服务器ID" required>
              <div class="derived-value">{{ deviceForm.domain_id || "-" }}</div>
            </el-form-item></el-col>
          <el-col :span="12"><el-form-item label="SIP域" required>
              <div class="derived-value">{{ deviceForm.domain || "-" }}</div>
            </el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="SIP服务器地址">
              <div class="derived-value">{{ sessionConfig.wan_ip || "-" }}</div>
            </el-form-item></el-col>
          <el-col :span="12"><el-form-item label="SIP服务器端口">
              <div class="derived-value">{{ sessionConfig.wan_port || "-" }}</div>
            </el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="设备别名"><el-input v-model="deviceForm.alias"
                :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="状态"><el-switch v-model="deviceForm.status" :active-value="1"
                :inactive-value="0" active-text="启用" inactive-text="停用"
                :disabled="deviceReadonly" /></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="密钥认证"><el-switch v-model="deviceForm.pwd_check" :active-value="1"
                :inactive-value="0" active-text="开启" inactive-text="关闭"
                :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="密钥"><el-input v-model="deviceForm.pwd"
                :disabled="deviceReadonly || deviceForm.pwd_check !== 1" /></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="心跳周期(秒)"><el-input-number v-model="deviceForm.heartbeat_sec" :min="5"
                :max="255" style="width: 100%" :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="地址"><el-input v-model="deviceForm.address"
                :disabled="deviceReadonly" /></el-form-item></el-col>
        </el-row>
        <el-row :gutter="16">
          <el-col :span="12"><el-form-item label="经度"><el-input v-model="deviceForm.longitude"
                :disabled="deviceReadonly" /></el-form-item></el-col>
          <el-col :span="12"><el-form-item label="纬度"><el-input v-model="deviceForm.latitude"
                :disabled="deviceReadonly" /></el-form-item></el-col>
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
      <template #footer><el-button @click="deviceDialog = false">取消</el-button><el-button v-if="!deviceReadonly"
          type="primary" :disabled="!canOperate" @click="saveDevice">保存</el-button></template>
    </el-dialog>

  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { createGbDevice, deleteGbDevice, getGbSessionNodeConfig, listGbDevicePage, listNodes, updateGbDevice, type GbDeviceInfo, type GbDevicePayload, type GbSessionConfigInfo, type NodeInfo } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const loading = ref(false);
const deviceId = ref('');
const deviceName = ref('');
const devices = ref<GbDeviceInfo[]>([]);
const page = ref(1);
const pageSize = ref(20);
const total = ref(0);
const sessionNodes = ref<NodeInfo[]>([]);
const sessionNodeOptions = ref<SessionNodeOption[]>([]);
const selectedListNodeId = ref('');
const listNodeLoading = ref(false);
const deviceDialog = ref(false);
const deviceReadonly = ref(false);
const sessionConfigLoading = ref(false);
const editingDevice = ref<GbDeviceInfo>();
const deviceForm = reactive<GbDevicePayload>(emptyDevice());
const sessionConfig = reactive({ wan_ip: "", wan_port: "" });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const deviceDialogTitle = computed(() => deviceReadonly.value ? "查看注册配置" : editingDevice.value ? "编辑注册配置" : "新增注册配置");
const selectedListNodeOption = computed(() => sessionNodeOptions.value.find((item) => item.node.node_id === selectedListNodeId.value));

type SessionNodeOption = { node: NodeInfo; config?: GbSessionConfigInfo; disabled: boolean; kindLabel: string; statusLabel: string };

function emptyDevice(): GbDevicePayload { return { device_id: '', alias: '', session_node_id: '', domain_id: '', domain: '', pwd_check: 1, pwd: '', status: 1, heartbeat_sec: 60, address: '', longitude: '', latitude: '', tenant_id: '', sys_org_code: '', create_by: '' }; }
function assign<T extends object>(target: T, source: Partial<T>) { Object.assign(target, source); }
function normalizeKind(value?: string | null) { return (value || '').trim().toLowerCase(); }
function nodeKindLabel(node: NodeInfo) { return (node.kind || node.service || node.config?.service || 'node').toUpperCase(); }
function nodeStatusLabel(disabled: boolean) { return disabled ? "离线" : "在线"; }
function buildSessionNodeOption(node: NodeInfo, config?: GbSessionConfigInfo): SessionNodeOption {
  const disabled = !isNodeOnline(node) || !config?.domain_id;
  return { node, config, disabled, kindLabel: nodeKindLabel(node), statusLabel: nodeStatusLabel(disabled) };
}
function isGbSessionNode(node: NodeInfo) { return normalizeKind(node.kind) === "session-gb28181" || normalizeKind(node.service) === "session-gb28181" || normalizeKind(node.protocol) === "gb28181"; }
function isNodeOnline(node?: NodeInfo) { return !!node && node.connection === "CONNECTED" && node.scheduling === "ENABLED"; }
function nodeLabel(node: NodeInfo) { return `${nodeKindLabel(node)} · ${node.node_id} · ${isNodeOnline(node) ? "在线" : "离线"}`; }
function listNodeLabel(option: SessionNodeOption) { return `${option.kindLabel} · ${option.node.node_id} · ${option.statusLabel}`; }
function clearSessionConfig(clearDomain = true) { if (clearDomain) { deviceForm.domain_id = ""; deviceForm.domain = ""; } sessionConfig.wan_ip = ""; sessionConfig.wan_port = ""; }
function applySessionConfig(config: GbSessionConfigInfo) { deviceForm.domain_id = config.domain_id; deviceForm.domain = config.domain; sessionConfig.wan_ip = config.wan_ip; sessionConfig.wan_port = String(config.wan_port || ""); }
async function loadSessionNodeConfig(nodeId: string, clearDomain = true, warn = true) {
  const node = sessionNodes.value.find((item) => item.node_id === nodeId);
  if (!isNodeOnline(node)) { clearSessionConfig(clearDomain); return false; }
  sessionConfigLoading.value = true;
  try {
    applySessionConfig(await getGbSessionNodeConfig(nodeId));
    return true;
  } catch (error) {
    clearSessionConfig(clearDomain);
    if (warn) ElMessage.error(error instanceof Error ? error.message : "Session 节点配置查询失败");
    return false;
  } finally {
    sessionConfigLoading.value = false;
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
async function loadDevices() { loading.value = true; try { await loadSessionNodes(); const option = selectedListNodeOption.value; if (!option || option.disabled || !option.config?.domain_id) { devices.value = []; total.value = 0; return; } const result = await listGbDevicePage(page.value, pageSize.value, option.node.node_id, option.config.domain_id, deviceId.value, deviceName.value); devices.value = result.items; total.value = result.total; page.value = result.page; pageSize.value = result.page_size; } catch (error) { ElMessage.error(error instanceof Error ? error.message : '设备加载失败'); } finally { loading.value = false; } }
async function queryDevices() { page.value = 1; await loadDevices(); }
async function resetDevices() { deviceId.value = ''; deviceName.value = ''; page.value = 1; await loadDevices(); }
async function handlePageSizeChange() { page.value = 1; await loadDevices(); }
async function handleListNodeChange() { page.value = 1; await loadDevices(); }
async function selectSessionNode(nodeId: string) { await loadSessionNodeConfig(nodeId); }
async function openDevice(row?: GbDeviceInfo, readonly = false) { deviceReadonly.value = readonly; editingDevice.value = row; clearSessionConfig(false); const payload: GbDevicePayload = row ? { device_id: row.device_id, session_node_id: row.session_node_id, domain_id: row.domain_id, domain: row.domain, longitude: row.longitude || "", latitude: row.latitude || "", address: row.address || "", pwd: row.pwd || "", pwd_check: row.pwd_check, alias: row.alias || "", status: row.status, heartbeat_sec: row.heartbeat_sec, tenant_id: row.tenant_id || "", sys_org_code: row.sys_org_code || "", create_by: row.create_by || "", update_by: row.update_by || "" } : emptyDevice(); assign(deviceForm, payload); deviceDialog.value = true; if (deviceForm.session_node_id) await loadSessionNodeConfig(deviceForm.session_node_id, !row, false); }
async function saveDevice() {
  const nodeId = deviceForm.session_node_id;
  if (!nodeId) return ElMessage.warning("Session 节点必填");
  const node = sessionNodes.value.find((item) => item.node_id === nodeId);
  if (!isNodeOnline(node)) return ElMessage.warning("所选 Session 节点离线，不能保存");
  if (sessionConfigLoading.value) return ElMessage.warning("Session 节点配置正在查询");
  const domain_id = deviceForm.domain_id || "";
  const domain = deviceForm.domain || "";
  if (!domain_id || !domain) return ElMessage.warning("所选 Session 节点缺少 domain_id/domain 配置");
  const payload = { ...deviceForm, domain_id, domain };
  try {
    await (editingDevice.value ? updateGbDevice(editingDevice.value.device_id, payload) : createGbDevice(payload));
    deviceDialog.value = false;
    await loadDevices();
    ElMessage.success("注册配置已保存");
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : "设备保存失败");
  }
}
async function removeDevice(row: GbDeviceInfo) { await ElMessageBox.confirm(`确认删除注册配置 ${row.device_id}？`, '删除确认', { type: 'warning' }); await deleteGbDevice(row.device_id, row.session_node_id, row.domain_id); if (devices.value.length === 1 && page.value > 1) page.value -= 1; await loadDevices(); ElMessage.success('注册配置已删除'); }
onMounted(loadDevices);
</script>

<style scoped>
.node-option {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  width: 100%;
}

.node-option.offline {
  color: var(--muted);
}

.node-status {
  font-size: 12px;
  color: var(--cyan);
}

.node-option.offline .node-status {
  color: var(--muted);
}

.pagination-bar {
  display: flex;
  justify-content: flex-end;
  padding-top: 14px;
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
</style>
