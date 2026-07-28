<template>
  <div class="page-grid viewport-card-page is-single-card-page" v-loading="loading">
    <GlassPanel class="span-12 fill-panel">
      <div class="toolbar stream-toolbar">
        <div class="stream-filter-row">
          <el-select v-model="sessionNodeId" placeholder="请选择 Session 节点（必选）" clearable filterable @change="resetAndLoad">
            <el-option v-for="node in sessionNodes" :key="node.node_id" :label="sessionNodeLabel(node)" :value="node.node_id" :disabled="!isNodeOnline(node)">
              <div class="node-option" :class="{ offline: !isNodeOnline(node) }">
                <span>{{ nodeKindLabel(node) }} · {{ node.node_id }}</span>
                <span class="node-status">{{ isNodeOnline(node) ? '在线' : '离线' }}</span>
              </div>
            </el-option>
          </el-select>
          <el-input v-model="filters.stream_id" placeholder="流 ID" clearable />
          <el-select v-model="filters.stream_node_id" placeholder="请选择流媒体服务" clearable filterable>
            <el-option v-for="node in streamNodes" :key="node.node_id" :label="node.display_name || node.node_id" :value="node.node_id" />
          </el-select>
          <el-input v-model="filters.device_id" placeholder="设备 ID" clearable />
        </div>
        <div class="stream-filter-row">
          <el-input v-model="filters.channel_id" placeholder="通道 ID" clearable />
          <el-input v-model="filters.ssrc" placeholder="SSRC" clearable />
          <el-select v-model="filters.state" placeholder="状态" clearable>
            <el-option v-for="state in stateOptions" :key="state.value" :label="state.label" :value="state.value" />
          </el-select>
          <div class="stream-filter-actions">
            <el-button type="primary" :disabled="!sessionNodeId" @click="search">查询</el-button>
            <el-button @click="resetFilters">重置</el-button>
          </div>
        </div>
      </div>

      <el-tabs class="stream-tabs" v-model="activeTab" @tab-change="tabChanged">
        <el-tab-pane label="当前运行" name="current">
          <el-table class="stream-table" :data="activeRows" empty-text="暂无当前运行资源" height="100%">
            <el-table-column prop="stream_id" label="流 ID" min-width="220" show-overflow-tooltip />
            <el-table-column prop="stream_node_id" label="流媒体服务" min-width="140" show-overflow-tooltip />
            <el-table-column prop="device_id" label="设备 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="channel_id" label="通道 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="ssrc" label="SSRC" width="120" />
            <el-table-column label="流类型" width="100"><template #default="{ row }">{{ streamTypeLabel(row.session_type) }}</template></el-table-column>
            <el-table-column label="会话状态" width="120"><template #default="{ row }"><StatusPill :label="statusLabel(row.dialog_state)" :tone="row.dialog_state.toLowerCase()" /></template></el-table-column>
            <el-table-column label="创建时间" width="180"><template #default="{ row }">{{ formatDateTime(row.created_at_ms) }}</template></el-table-column>
            <el-table-column label="持续时间" width="120"><template #default="{ row }">{{ formatDuration(Math.max(0, serverTimeMs - row.started_at_ms)) }}</template></el-table-column>
            <el-table-column label="操作" fixed="right" width="100">
              <template #default="{ row }">
                <el-button link type="primary" :loading="managementLoadingStreamId === row.stream_id" @click="openManagement(row)">管理</el-button>
              </template>
            </el-table-column>
          </el-table>
          <div class="pager-row">
            <el-pagination v-model:current-page="currentPage" v-model:page-size="currentPageSize" :total="currentTotal"
              :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next, jumper"
              @current-change="loadCurrentPage" @size-change="handleCurrentPageSizeChange" />
          </div>
        </el-tab-pane>

        <el-tab-pane label="历史记录" name="history">
          <el-table class="stream-table" :data="historyRows" empty-text="暂无历史记录" height="100%">
            <el-table-column prop="stream_id" label="流 ID" min-width="220" show-overflow-tooltip />
            <el-table-column prop="stream_node_id" label="流媒体服务" min-width="140" show-overflow-tooltip />
            <el-table-column prop="device_id" label="设备 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="channel_id" label="通道 ID" min-width="170" show-overflow-tooltip />
            <el-table-column prop="ssrc" label="SSRC" width="120" />
            <el-table-column label="流类型" width="100"><template #default="{ row }">{{ streamTypeLabel(row.session_type) }}</template></el-table-column>
            <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="statusLabel(row.state)" :tone="row.state.toLowerCase()" /></template></el-table-column>
            <el-table-column label="创建时间" width="180"><template #default="{ row }">{{ formatDateTime(row.created_at_ms) }}</template></el-table-column>
            <el-table-column label="结束时间" width="180"><template #default="{ row }">{{ formatDateTime(row.terminated_at_ms) }}<span v-if="row.legacy_terminal_time"> *</span></template></el-table-column>
            <el-table-column label="持续时间" width="120"><template #default="{ row }">{{ formatDuration(row.duration_ms) }}</template></el-table-column>
            <el-table-column label="停止原因" min-width="220"><template #default="{ row }"><div>{{ row.terminal_reason_label || '-' }}</div><small v-if="row.stop_reason">操作原因：{{ row.stop_reason }}</small></template></el-table-column>
            <el-table-column label="失败码" min-width="150"><template #default="{ row }">{{ row.error_code || '-' }}</template></el-table-column>
          </el-table>
          <div class="pager-row">
            <span v-if="historyRows.some((row) => row.legacy_terminal_time)">* 为旧数据兼容时间</span>
            <el-pagination v-model:current-page="historyPage" v-model:page-size="historyPageSize" :total="historyTotal"
              :page-sizes="[10, 20, 50, 100]" layout="total, sizes, prev, pager, next, jumper"
              @current-change="loadHistoryPage" @size-change="handleHistoryPageSizeChange" />
          </div>
        </el-tab-pane>
      </el-tabs>
    </GlassPanel>

    <el-dialog v-model="managementVisible" title="流管理" width="680px" destroy-on-close>
      <template v-if="managementRow">
        <el-descriptions :column="2" border>
          <el-descriptions-item label="流 ID" :span="2">{{ managementRow.stream_id }}</el-descriptions-item>
          <el-descriptions-item label="流类型">{{ streamTypeLabel(managementRow.session_type) }}</el-descriptions-item>
          <el-descriptions-item label="实时状态">{{ statusLabel(managementRow.state) }}</el-descriptions-item>
          <el-descriptions-item label="会话状态">{{ statusLabel(managementRow.dialog_state) }}</el-descriptions-item>
          <el-descriptions-item label="媒体状态">{{ managementRow.media_state || '-' }}</el-descriptions-item>
          <el-descriptions-item label="设备 ID">{{ managementRow.device_id || '-' }}</el-descriptions-item>
          <el-descriptions-item label="通道 ID">{{ managementRow.channel_id || '-' }}</el-descriptions-item>
          <el-descriptions-item label="SSRC">{{ managementRow.ssrc || '-' }}</el-descriptions-item>
          <el-descriptions-item label="流媒体服务">{{ managementRow.stream_node_id || '-' }}</el-descriptions-item>
          <el-descriptions-item label="诊断原因" :span="2">{{ managementRow.diagnostic_reason || '-' }}</el-descriptions-item>
          <el-descriptions-item v-if="isDownloadDetail" label="下载格式" :span="2">{{ managementRow.output_format ? mediaFormatLabel(managementRow.output_format) : '-' }}</el-descriptions-item>
        </el-descriptions>
        <el-alert v-if="managementMediaStopped" class="management-alert" type="warning" :closable="false" title="媒体已停止，会话尚未收敛，可使用强制停止完成关闭。" />
        <section v-if="!isDownloadDetail" class="stream-detail-section">
          <h4>观看概览</h4>
          <el-descriptions :column="1" border>
            <el-descriptions-item label="总观看人数">{{ managementRow.viewer_count }}</el-descriptions-item>
          </el-descriptions>
        </section>
        <section v-if="!isDownloadDetail" class="stream-detail-section">
          <h4>媒体格式</h4>
          <el-table :data="detailViewerFormats" border empty-text="暂无支持的媒体格式">
            <el-table-column label="媒体格式"><template #default="{ row }">{{ mediaFormatLabel(row.media_format) }}</template></el-table-column>
            <el-table-column prop="viewer_count" label="观看人数" width="140" align="right" />
          </el-table>
        </section>
        <el-form v-if="canOperate" class="management-form" label-position="top">
          <el-form-item label="停止原因" required>
            <el-input v-model="stopReason" type="textarea" :rows="3" maxlength="255" show-word-limit placeholder="请输入强制停止原因" />
          </el-form-item>
        </el-form>
      </template>
      <template #footer>
        <el-button @click="managementVisible = false">关闭</el-button>
        <el-button v-if="canOperate" type="danger" :loading="stopSubmitting" :disabled="!stopReason.trim()" @click="forceStop">强制停止</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage, ElMessageBox } from 'element-plus';
import { ApiError, errorMessage, getActiveStreamManagement, listActiveStreamMonitor, listNodes, listStreamHistoryMonitor, stopMonitoredStream, type ActiveStreamDialogItem, type ActiveStreamMonitorItem, type NodeInfo, type StreamHistoryMonitorItem, type StreamMonitorQuery } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import { useAuthStore } from '@/stores/auth';
import { formatDateTime } from '@/utils/dateTime';

const auth = useAuthStore();
const activeTab = ref<'current' | 'history'>('current');
const nodes = ref<NodeInfo[]>([]);
const sessionNodeId = ref('');
const loading = ref(false);
const activeRows = ref<ActiveStreamDialogItem[]>([]);
const historyRows = ref<StreamHistoryMonitorItem[]>([]);
const currentPage = ref(1);
const currentPageSize = ref(20);
const currentTotal = ref(0);
const historyPage = ref(1);
const historyPageSize = ref(20);
const historyTotal = ref(0);
const serverTimeMs = ref(Date.now());
const managementVisible = ref(false);
const managementRow = ref<ActiveStreamMonitorItem>();
const managementLoadingStreamId = ref('');
const stopReason = ref('');
const stopSubmitting = ref(false);
const isDownloadDetail = computed(() => managementRow.value?.session_type.trim().toUpperCase() === 'DOWNLOAD');
const managementMediaStopped = computed(() => managementRow.value?.media_state === 'stopped' || managementRow.value?.diagnostic_reason === 'media_not_running');
const detailViewerFormats = computed(() => {
  const row = managementRow.value;
  if (!row || isDownloadDetail.value) return [];
  const viewers = new Map(row.viewer_formats.map((item) => [normalizeMediaFormat(item.media_format), item.viewer_count]));
  const formats = row.supported_formats.length > 0 ? row.supported_formats : row.viewer_formats.map((item) => item.media_format);
  return formats.map((format) => ({ media_format: format, viewer_count: viewers.get(normalizeMediaFormat(format)) || 0 }));
});
const requestInFlight = ref(false);
const filters = reactive<Required<StreamMonitorQuery>>({ stream_id: '', stream_node_id: '', device_id: '', channel_id: '', ssrc: '', state: '' });
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const sessionNodes = computed(() => nodes.value
  .filter(isGbSessionNode)
  .sort((left, right) => Number(!isNodeOnline(left)) - Number(!isNodeOnline(right)) || left.node_id.localeCompare(right.node_id)));
const streamNodes = computed(() => nodes.value.filter(isStreamNode));
const stateOptions = computed(() => activeTab.value === 'current'
  ? [
      { value: 'INVITING', label: '建立中' },
      { value: 'ESTABLISHED', label: '已建立' },
      { value: 'TERMINATING', label: '关闭中' },
    ]
  : [
      { value: 'TERMINATED', label: '已终止' },
      { value: 'ORPHAN', label: '异常终止' },
    ]);

function query(): StreamMonitorQuery { return { ...filters }; }
function normalizeKind(value?: string | null) { return (value || '').trim().toLowerCase(); }
function nodeKindLabel(node: NodeInfo) { return (node.kind || node.service || node.config?.service || 'node').toUpperCase(); }
function isGbSessionNode(node: NodeInfo) { return normalizeKind(node.kind) === 'session-gb28181' || normalizeKind(node.service) === 'session-gb28181' || normalizeKind(node.protocol) === 'gb28181'; }
function isStreamNode(node: NodeInfo) { return normalizeKind(node.kind) === 'stream' || normalizeKind(node.service) === 'stream'; }
function isNodeOnline(node: NodeInfo) { return node.connection === 'CONNECTED' && node.scheduling === 'ENABLED'; }
function sessionNodeLabel(node: NodeInfo) { return `${nodeKindLabel(node)} · ${node.node_id} · ${isNodeOnline(node) ? '在线' : '离线'}`; }
function statusLabel(state: string): string { return ({ starting: '启动中', running: '运行中', stopping: '停止中', failed: '失败', unknown: '未知', conflict: '冲突', INVITING: '建立中', ESTABLISHED: '已建立', TERMINATING: '关闭中', TERMINATED: '已终止', ORPHAN: '异常终止' } as Record<string, string>)[state] || state; }
function streamTypeLabel(value: string): string { return ({ LIVE: '直播', PLAYBACK: '回放', DOWNLOAD: '下载', TALK: '语音' } as Record<string, string>)[value.trim().toUpperCase()] || '未知'; }
function normalizeMediaFormat(value: string): string { return value.trim().toLowerCase(); }
function mediaFormatLabel(value: string): string { return ({ flv: 'HTTP-FLV', http_flv: 'HTTP-FLV', fmp4: 'fMP4', dash_fmp4: 'fMP4', hls: 'HLS', hls_fmp4: 'HLS', ll_hls: 'LL-HLS', mp4: 'MP4' } as Record<string, string>)[normalizeMediaFormat(value)] || value.toUpperCase(); }
function formatDuration(ms: number): string { const seconds = Math.max(0, Math.floor(ms / 1000)); const hours = Math.floor(seconds / 3600); const minutes = Math.floor(seconds % 3600 / 60); const remain = seconds % 60; return hours > 0 ? `${hours}时${minutes}分${remain}秒` : minutes > 0 ? `${minutes}分${remain}秒` : `${remain}秒`; }
function clearRows() { activeRows.value = []; historyRows.value = []; currentTotal.value = 0; historyTotal.value = 0; }
function resetPagination() { currentPage.value = 1; historyPage.value = 1; }
function resetActivePage() { if (activeTab.value === 'current') currentPage.value = 1; else historyPage.value = 1; }
async function load(showLoading = true) {
  if (!sessionNodeId.value) { clearRows(); return; }
  if (requestInFlight.value) return;
  const requestedSession = sessionNodeId.value;
  const requestedTab = activeTab.value;
  let retryLastPage = false;
  requestInFlight.value = true;
  if (showLoading) loading.value = true;
  try {
    if (requestedTab === 'current') {
      const page = await listActiveStreamMonitor(requestedSession, query(), currentPage.value, currentPageSize.value);
      if (sessionNodeId.value !== requestedSession || activeTab.value !== requestedTab) return;
      activeRows.value = page.items; currentTotal.value = page.total; serverTimeMs.value = page.server_time_ms;
      const lastPage = Math.max(1, Math.ceil(page.total / currentPageSize.value));
      if (currentPage.value > lastPage) { currentPage.value = lastPage; retryLastPage = true; }
    } else {
      const page = await listStreamHistoryMonitor(requestedSession, query(), historyPage.value, historyPageSize.value);
      if (sessionNodeId.value !== requestedSession || activeTab.value !== requestedTab) return;
      historyRows.value = page.items; historyTotal.value = page.total; serverTimeMs.value = page.server_time_ms;
      const lastPage = Math.max(1, Math.ceil(page.total / historyPageSize.value));
      if (historyPage.value > lastPage) { historyPage.value = lastPage; retryLastPage = true; }
    }
  } catch (error) { clearRows(); ElMessage.error(errorMessage(error, '流监控数据加载失败')); }
  finally { requestInFlight.value = false; if (showLoading) loading.value = false; }
  if (retryLastPage) await load(showLoading);
}
function search() { resetActivePage(); load(); }
function resetFilters() { Object.assign(filters, { stream_id: '', stream_node_id: '', device_id: '', channel_id: '', ssrc: '', state: '' }); search(); }
function resetAndLoad() { resetPagination(); clearRows(); load(); }
function tabChanged() {
  filters.state = '';
  if (activeTab.value === 'current') currentPage.value = 1;
  else historyPage.value = 1;
  clearRows();
  load();
}
function loadCurrentPage() { load(); }
function loadHistoryPage() { load(); }
function handleCurrentPageSizeChange() { currentPage.value = 1; load(); }
function handleHistoryPageSizeChange() { historyPage.value = 1; load(); }
async function openManagement(row: ActiveStreamDialogItem) {
  const requestedSession = sessionNodeId.value;
  managementLoadingStreamId.value = row.stream_id;
  try {
    const result = await getActiveStreamManagement(requestedSession, row.stream_id);
    if (sessionNodeId.value !== requestedSession) return;
    if (result.state === 'ended') {
      ElMessage.warning(result.ended?.state === 'ORPHAN' ? '该流已异常结束，当前列表已刷新' : '该流已结束，当前列表已刷新');
      await load(false);
      return;
    }
    if (!result.active) throw new Error('active stream management item is missing');
    managementRow.value = result.active;
    stopReason.value = '';
    managementVisible.value = true;
  } catch (error) {
    if (error instanceof ApiError && error.status === 404) {
      ElMessage.warning('该流已结束或记录不存在，当前列表已刷新');
      await load(false);
    } else {
      ElMessage.error(errorMessage(error, '流管理信息加载失败'));
    }
  } finally {
    if (managementLoadingStreamId.value === row.stream_id) managementLoadingStreamId.value = '';
  }
}
async function forceStop() {
  const row = managementRow.value;
  const reason = stopReason.value.trim();
  if (!row || !reason) { ElMessage.warning('请输入停止原因'); return; }
  try {
    await ElMessageBox.confirm(`确认强制停止设备媒体流 ${row.stream_id}？\n停止原因：${reason}`, '强制停止当前流', { type: 'warning', confirmButtonText: '确认强制停止', cancelButtonText: '取消' });
    stopSubmitting.value = true;
    const response = await stopMonitoredStream(row.session_node_id, row.stream_id, `ui-stream-stop-${Date.now()}`, reason);
    ElMessage.success(response.state === 'stopping' ? '停止请求已受理' : '流已处于终态');
    managementVisible.value = false;
    await load(false);
  } catch (error) { if (error !== 'cancel' && error !== 'close') ElMessage.error(errorMessage(error, '强制停止失败')); }
  finally { stopSubmitting.value = false; }
}
onMounted(async () => { try { nodes.value = await listNodes(); } catch (error) { ElMessage.error(errorMessage(error, 'Session 节点加载失败')); } });
</script>

<style scoped>
.stream-toolbar { display: flex; flex-direction: column; align-items: stretch; gap: 10px; width: 100%; box-sizing: border-box; padding: 0 16px; margin-bottom: 14px; }
.stream-filter-row { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; width: 100%; }
.stream-filter-row .el-input, .stream-filter-row .el-select { min-width: 0; width: 100%; }
.stream-filter-actions { display: flex; align-items: center; gap: 10px; }
.node-option { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
.node-option.offline { color: var(--el-text-color-secondary); }
.node-status { flex: none; font-size: 12px; }
.pager-row { display: flex; align-items: center; justify-content: flex-end; gap: 12px; min-height: 48px; color: var(--el-text-color-secondary); }
.stream-tabs { display: flex; flex: 1; flex-direction: column; min-height: 0; }
.stream-tabs :deep(.el-tabs__content) { flex: 1; min-height: 0; }
.stream-tabs :deep(.el-tab-pane) { display: flex; flex-direction: column; height: 100%; min-height: 0; }
.stream-table { flex: 1; min-height: 0; }
.stream-detail-section { margin-top: 18px; }
.stream-detail-section h4 { margin: 0 0 10px; font-size: 14px; font-weight: 600; }
@media (max-width: 1000px) { .stream-filter-row { grid-template-columns: repeat(2, minmax(0, 1fr)); width: 100%; } }
@media (max-width: 900px) {
  .stream-tabs,
  .stream-tabs :deep(.el-tab-pane) { display: block; height: auto; }
  .stream-table { height: 520px; }
}
</style>
