<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-12" title="当前运行资源监控" subtitle="业务事实来自所选 Session，Guard 仅鉴权与转发">
      <div class="toolbar stream-toolbar">
        <el-select v-model="sessionNodeId" placeholder="请选择 Session 节点（必选）" clearable filterable style="width: 280px" @change="resetAndLoad">
          <el-option v-for="node in sessionNodes" :key="node.instance_id" :label="node.display_name || node.node_id" :value="node.node_id">
            <span>{{ node.display_name || node.node_id }}</span>
            <span class="node-instance">{{ node.instance_id }}</span>
          </el-option>
        </el-select>
        <el-input v-model="filters.stream_id" placeholder="流 ID" clearable />
        <el-input v-model="filters.stream_node_id" placeholder="流媒体服务标识" clearable />
        <el-input v-model="filters.device_id" placeholder="设备 ID" clearable />
        <el-input v-model="filters.channel_id" placeholder="通道 ID" clearable />
        <el-input v-model="filters.ssrc" placeholder="SSRC" clearable />
        <el-select v-model="filters.state" placeholder="状态" clearable style="width: 150px">
          <el-option v-for="state in stateOptions" :key="state.value" :label="state.label" :value="state.value" />
        </el-select>
        <el-button type="primary" :disabled="!sessionNodeId" @click="search">查询</el-button>
        <el-button @click="resetFilters">重置</el-button>
        <el-button :disabled="!sessionNodeId" @click="load">刷新</el-button>
      </div>

      <el-alert v-if="!sessionNodeId" title="请先选择 Session 节点；页面不会从 Guard route/lease 推断当前业务流。" type="info" :closable="false" show-icon />

      <el-tabs v-model="activeTab" @tab-change="tabChanged">
        <el-tab-pane label="当前运行" name="current">
          <el-table :data="activeRows" empty-text="暂无当前运行资源" height="520">
            <el-table-column prop="stream_id" label="流 ID" min-width="220" show-overflow-tooltip />
            <el-table-column prop="session_node_id" label="Session 服务" min-width="190" show-overflow-tooltip />
            <el-table-column prop="stream_node_id" label="流媒体服务" min-width="140" show-overflow-tooltip />
            <el-table-column prop="device_id" label="设备 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="channel_id" label="通道 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="ssrc" label="SSRC" width="120" />
            <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.state.toUpperCase()" :tone="row.state" /></template></el-table-column>
            <el-table-column label="创建时间" width="180"><template #default="{ row }">{{ formatDateTime(row.created_at_ms) }}</template></el-table-column>
            <el-table-column label="持续时间" width="120"><template #default="{ row }">{{ formatDuration(Math.max(0, serverTimeMs - row.started_at_ms)) }}</template></el-table-column>
            <el-table-column prop="viewer_count" label="观看人数" width="100" />
            <el-table-column label="媒体格式" min-width="180"><template #default="{ row }">{{ formatViewerFormats(row) }}</template></el-table-column>
            <el-table-column label="诊断" min-width="160"><template #default="{ row }">{{ row.diagnostic_reason || '-' }}</template></el-table-column>
            <el-table-column label="操作" fixed="right" width="100"><template #default="{ row }"><el-button link type="danger" :disabled="!canOperate || row.state === 'stopping'" @click="stop(row)">停止</el-button></template></el-table-column>
          </el-table>
          <div class="pager-row">
            <span v-if="activeRows.length === 0 && nextAfterId">本批无匹配，仍有后续数据</span>
            <span v-else>本页 {{ activeRows.length }} 条</span>
            <el-button :disabled="cursorStack.length === 0" @click="previousCurrent">上一页</el-button>
            <el-button :disabled="!nextAfterId" @click="nextCurrent">下一页</el-button>
          </div>
        </el-tab-pane>

        <el-tab-pane label="历史记录" name="history">
          <el-table :data="historyRows" empty-text="暂无历史记录" height="520">
            <el-table-column prop="stream_id" label="流 ID" min-width="220" show-overflow-tooltip />
            <el-table-column prop="session_node_id" label="Session 服务" min-width="190" show-overflow-tooltip />
            <el-table-column prop="stream_node_id" label="流媒体服务" min-width="140" show-overflow-tooltip />
            <el-table-column prop="device_id" label="设备 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="channel_id" label="通道 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="ssrc" label="SSRC" width="120" />
            <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.state" :tone="row.state.toLowerCase()" /></template></el-table-column>
            <el-table-column label="创建时间" width="180"><template #default="{ row }">{{ formatDateTime(row.created_at_ms) }}</template></el-table-column>
            <el-table-column label="结束时间" width="180"><template #default="{ row }">{{ formatDateTime(row.terminated_at_ms) }}<span v-if="row.legacy_terminal_time"> *</span></template></el-table-column>
            <el-table-column label="持续时间" width="120"><template #default="{ row }">{{ formatDuration(row.duration_ms) }}</template></el-table-column>
            <el-table-column prop="terminal_reason" label="停止原因" min-width="150" />
            <el-table-column label="失败码" min-width="150"><template #default="{ row }">{{ row.error_code || '-' }}</template></el-table-column>
          </el-table>
          <div class="pager-row">
            <span>共 {{ historyTotal }} 条<span v-if="historyRows.some((row) => row.legacy_terminal_time)">；* 为旧数据兼容时间</span></span>
            <el-pagination v-model:current-page="historyPage" :page-size="pageSize" :total="historyTotal" layout="prev, pager, next" @current-change="load" />
          </div>
        </el-tab-pane>
      </el-tabs>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { errorMessage, listActiveStreamMonitor, listNodes, listStreamHistoryMonitor, stopMonitoredStream, type ActiveStreamMonitorItem, type NodeInfo, type StreamHistoryMonitorItem, type StreamMonitorQuery } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { useAuthStore } from '@/stores/auth';
import { formatDateTime } from '@/utils/dateTime';

const auth = useAuthStore();
const activeTab = ref<'current' | 'history'>('current');
const nodes = ref<NodeInfo[]>([]);
const sessionNodeId = ref('');
const loading = ref(false);
const activeRows = ref<ActiveStreamMonitorItem[]>([]);
const historyRows = ref<StreamHistoryMonitorItem[]>([]);
const nextAfterId = ref('');
const currentCursor = ref('');
const cursorStack = ref<string[]>([]);
const historyPage = ref(1);
const historyTotal = ref(0);
const serverTimeMs = ref(Date.now());
const pageSize = 20;
const filters = reactive<Required<StreamMonitorQuery>>({ stream_id: '', stream_node_id: '', device_id: '', channel_id: '', ssrc: '', state: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const sessionNodes = computed(() => nodes.value.filter((node) => node.kind.toLowerCase() === 'session' && node.protocol?.toLowerCase() === 'gb28181' && node.connection.toLowerCase() === 'connected'));
const stateOptions = computed(() => activeTab.value === 'current'
  ? ['starting', 'running', 'stopping', 'failed', 'unknown', 'conflict'].map((value) => ({ value, label: value.toUpperCase() }))
  : ['TERMINATED', 'ORPHAN'].map((value) => ({ value, label: value })));

function query(): StreamMonitorQuery { return { ...filters }; }
function formatDuration(ms: number): string { const seconds = Math.max(0, Math.floor(ms / 1000)); const hours = Math.floor(seconds / 3600); const minutes = Math.floor(seconds % 3600 / 60); const remain = seconds % 60; return hours > 0 ? `${hours}时${minutes}分${remain}秒` : minutes > 0 ? `${minutes}分${remain}秒` : `${remain}秒`; }
function formatViewerFormats(row: ActiveStreamMonitorItem): string { return row.viewer_formats.length > 0 ? row.viewer_formats.map((item) => `${item.media_format.toUpperCase()} ${item.viewer_count}`).join(' / ') : '-'; }
function clearRows() { activeRows.value = []; historyRows.value = []; nextAfterId.value = ''; historyTotal.value = 0; }
function resetPagination() { currentCursor.value = ''; cursorStack.value = []; historyPage.value = 1; }
async function load() {
  if (!sessionNodeId.value) { clearRows(); return; }
  loading.value = true;
  try {
    if (activeTab.value === 'current') {
      const page = await listActiveStreamMonitor(sessionNodeId.value, query(), currentCursor.value, pageSize);
      activeRows.value = page.items; nextAfterId.value = page.next_after_id; serverTimeMs.value = page.server_time_ms;
    } else {
      const page = await listStreamHistoryMonitor(sessionNodeId.value, query(), historyPage.value, pageSize);
      historyRows.value = page.items; historyTotal.value = page.total; serverTimeMs.value = page.server_time_ms;
      const lastPage = Math.max(1, Math.ceil(page.total / pageSize));
      if (historyPage.value > lastPage) { historyPage.value = lastPage; await load(); }
    }
  } catch (error) { clearRows(); ElMessage.error(errorMessage(error, '流监控数据加载失败')); }
  finally { loading.value = false; }
}
function search() { resetPagination(); load(); }
function resetFilters() { Object.assign(filters, { stream_id: '', stream_node_id: '', device_id: '', channel_id: '', ssrc: '', state: '' }); search(); }
function resetAndLoad() { resetPagination(); clearRows(); load(); }
function tabChanged() { filters.state = ''; resetPagination(); clearRows(); load(); }
function nextCurrent() { if (!nextAfterId.value) return; cursorStack.value.push(currentCursor.value); currentCursor.value = nextAfterId.value; load(); }
function previousCurrent() { const cursor = cursorStack.value.pop(); if (cursor === undefined) return; currentCursor.value = cursor; load(); }
async function stop(row: ActiveStreamMonitorItem) {
  try {
    await ElMessageBox.confirm(`确认向设备补偿停止流 ${row.stream_id}？`, '停止当前流', { type: 'warning', confirmButtonText: '确认停止', cancelButtonText: '取消' });
    const response = await stopMonitoredStream(sessionNodeId.value, row.stream_id, `ui-stream-stop-${Date.now()}`);
    ElMessage.success(response.state === 'stopping' ? '停止请求已受理' : '流已处于终态');
    await load();
  } catch (error) { if (error !== 'cancel' && error !== 'close') ElMessage.error(errorMessage(error, '停止失败')); }
}
const clockTimer = window.setInterval(() => { serverTimeMs.value += 1000; }, 1000);
onBeforeUnmount(() => window.clearInterval(clockTimer));
onMounted(async () => { try { nodes.value = await listNodes(); } catch (error) { ElMessage.error(errorMessage(error, 'Session 节点加载失败')); } });
</script>

<style scoped>
.stream-toolbar { display: grid; grid-template-columns: minmax(240px, 1.4fr) repeat(5, minmax(130px, 1fr)) 150px auto auto auto; gap: 10px; margin-bottom: 14px; }
.stream-toolbar .el-input { min-width: 0; }
.node-instance { float: right; margin-left: 16px; color: var(--el-text-color-secondary); font-size: 12px; }
.pager-row { display: flex; align-items: center; justify-content: flex-end; gap: 12px; min-height: 48px; color: var(--el-text-color-secondary); }
@media (max-width: 1400px) { .stream-toolbar { grid-template-columns: repeat(4, minmax(150px, 1fr)); } }
</style>
