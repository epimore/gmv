<template>
  <div class="page-grid viewport-card-page has-summary-cards" v-loading="loading">
    <MetricCard class="span-3" label="就绪节点" :value="readyCount" trend="可调度" :hint="`${nodes.length} 个节点`" />
    <MetricCard class="span-3" label="异常节点" :value="warningCount" trend="需关注" hint="健康或调度异常" />
    <MetricCard class="span-3" label="离线节点" :value="offlineCount" trend="连接中断" hint="离线" />
    <MetricCard class="span-3" label="当前任务" :value="currentTaskCount" :trend="`执行 ${runningTaskCount}`"
      :hint="`排队 ${queuedTaskCount}`" />

    <GlassPanel class="span-12 fill-panel">
      <div class="health-tabs-wrap">
        <el-button class="health-refresh" :loading="loading" @click="load">刷新</el-button>
        <el-tabs v-model="activeTab">
          <el-tab-pane label="任务拥堵" name="tasks">
            <div class="health-filter-scroll">
              <el-form class="health-filter-row task-filters" :inline="true" :model="taskFilterDraft"
                aria-label="任务拥堵查询条件">
                <el-form-item label="类型">
                  <el-select v-model="taskFilterDraft.kind" clearable placeholder="全部类型">
                    <el-option v-for="kind in kindOptions" :key="kind" :label="kind" :value="kind" />
                  </el-select>
                </el-form-item>
                <el-form-item label="监控状态">
                  <el-select v-model="taskFilterDraft.health" clearable placeholder="全部状态">
                    <el-option v-for="health in healthOptions" :key="health.value" :label="health.label"
                      :value="health.value" />
                  </el-select>
                </el-form-item>
                <el-form-item label="节点 ID">
                  <el-select v-model="taskFilterDraft.nodeId" clearable filterable placeholder="输入节点 ID 模糊匹配">
                    <el-option v-for="nodeId in nodeIdOptions" :key="nodeId" :label="nodeId" :value="nodeId" />
                  </el-select>
                </el-form-item>
                <el-form-item class="filter-actions">
                  <el-button type="primary" @click="queryTaskFilters">查询</el-button>
                  <el-button @click="resetTaskFilters">重置</el-button>
                </el-form-item>
              </el-form>
            </div>

            <div class="health-tab-body">
              <div v-if="taskPageRows.length" class="node-load-list" role="list" aria-label="节点任务负载">
                <article v-for="item in taskPageRows" :key="item.node.node_id" class="node-load-row"
                  :class="`is-${item.alertLevel}`" :data-node-id="item.node.node_id"
                  :data-alert-level="item.alertLevel" role="listitem">
                  <div class="node-load-identity">
                    <b class="code">{{ item.node.node_id }}</b>
                    <small>{{ item.node.kind }}</small>
                  </div>
                  <div class="node-load-state">
                    <StatusPill :label="item.node.health_label" :tone="item.node.health" />
                    <span :class="{ warning: item.node.scheduling !== 'ENABLED' }">{{ item.node.scheduling_label }}</span>
                  </div>
                  <div class="node-load-visual">
                    <div class="node-load-track" role="img"
                      :aria-label="`${item.node.node_id}：执行中 ${item.running}，排队中 ${item.queued}，合计 ${item.total}`">
                      <span v-if="item.running" class="node-load-running" :style="{ width: taskWidth(item.running) }" />
                      <span v-if="item.queued" class="node-load-queued" :style="{ width: taskWidth(item.queued) }" />
                    </div>
                    <div class="node-load-counts">
                      <span class="running">执行 {{ item.running }}</span>
                      <span class="queued">排队 {{ item.queued }}</span>
                      <b>合计 {{ item.total }}</b>
                    </div>
                  </div>
                </article>
              </div>
              <el-empty v-else description="暂无符合条件的节点" />
            </div>
            <div class="pager-row">
              <el-pagination v-model:current-page="taskPage" :page-size="PAGE_SIZE" :total="filteredTaskLoads.length"
                layout="total, prev, pager, next" />
            </div>
          </el-tab-pane>

          <el-tab-pane label="节点矩阵" name="matrix">
            <div class="health-filter-scroll">
              <el-form class="health-filter-row matrix-filters" :inline="true" :model="matrixFilterDraft"
                aria-label="节点矩阵查询条件">
                <el-form-item label="类型">
                  <el-select v-model="matrixFilterDraft.kind" clearable placeholder="全部类型">
                    <el-option v-for="kind in kindOptions" :key="kind" :label="kind" :value="kind" />
                  </el-select>
                </el-form-item>
                <el-form-item label="监控状态">
                  <el-select v-model="matrixFilterDraft.health" clearable placeholder="全部状态">
                    <el-option v-for="health in healthOptions" :key="health.value" :label="health.label"
                      :value="health.value" />
                  </el-select>
                </el-form-item>
                <el-form-item label="节点 ID">
                  <el-select v-model="matrixFilterDraft.nodeId" clearable filterable placeholder="输入节点 ID 模糊匹配">
                    <el-option v-for="nodeId in nodeIdOptions" :key="nodeId" :label="nodeId" :value="nodeId" />
                  </el-select>
                </el-form-item>
                <el-form-item class="filter-actions">
                  <el-button type="primary" @click="queryMatrixFilters">查询</el-button>
                  <el-button @click="resetMatrixFilters">重置</el-button>
                </el-form-item>
              </el-form>
            </div>

            <div class="health-tab-body">
              <el-table class="node-matrix-table" :data="matrixPageRows" height="100%" highlight-current-row
                empty-text="暂无符合条件的节点" @row-click="openNodeDetail">
                <el-table-column prop="node_id" label="节点 ID" min-width="170" show-overflow-tooltip />
                <el-table-column prop="kind" label="类型" min-width="130" show-overflow-tooltip />
                <el-table-column label="协议" min-width="110" show-overflow-tooltip><template #default="{ row }">{{ row.protocol || '-'
                    }}</template></el-table-column>
                <el-table-column label="健康" min-width="110"><template #default="{ row }">
                    <StatusPill :label="row.health_label" :tone="row.health" />
                  </template></el-table-column>
                <el-table-column label="CPU" min-width="145"><template #default="{ row }"><el-progress
                      :percentage="Math.round(row.host_metrics.cpu_usage_percent)" /></template></el-table-column>
                <el-table-column label="内存" min-width="145"><template #default="{ row }"><el-progress
                      :percentage="memoryPercent(row)" /></template></el-table-column>
                <el-table-column label="磁盘 IO" min-width="180"><template #default="{ row }"><span class="io-cell">↓{{
                  formatRate(row.host_metrics.disk_read_bytes_per_sec) }} ↑{{
                      formatRate(row.host_metrics.disk_write_bytes_per_sec) }}</span></template></el-table-column>
                <el-table-column label="网络 IO" min-width="180"><template #default="{ row }"><span class="io-cell">↓{{
                  formatRate(row.host_metrics.network_receive_bytes_per_sec) }} ↑{{
                      formatRate(row.host_metrics.network_transmit_bytes_per_sec) }}</span></template></el-table-column>
              </el-table>
            </div>
            <div class="pager-row">
              <el-pagination v-model:current-page="matrixPage" :page-size="PAGE_SIZE" :total="filteredMatrixNodes.length"
                layout="total, prev, pager, next" />
            </div>
          </el-tab-pane>
        </el-tabs>
      </div>
    </GlassPanel>

    <el-drawer v-model="nodeDetailVisible" :title="nodeDetailTitle" size="min(520px, 100vw)" destroy-on-close>
      <div v-if="selected" class="node-detail">
        <div class="node-detail-grid">
          <div class="node-detail-field is-wide"><span class="node-detail-label">节点 ID</span><b
              class="node-detail-value code">{{ selected.node_id }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">类型</span><b
              class="node-detail-value">{{ selected.kind || '-' }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">协议</span><b
              class="node-detail-value">{{ selected.protocol || '-' }}</b></div>
          <div class="node-detail-field"><span class="node-detail-label">健康</span><div class="node-detail-value">
              <StatusPill :label="selected.health_label" :tone="selected.health" />
            </div></div>
          <div class="node-detail-field"><span class="node-detail-label">连接</span><b
              class="node-detail-value">{{ selected.connection_label }}</b></div>
          <div class="node-detail-field"><span class="node-detail-label">调度</span><b
              class="node-detail-value">{{ selected.scheduling_label }}</b></div>
          <div class="node-detail-field"><span class="node-detail-label">CPU</span><b
              class="node-detail-value">{{ selected.host_metrics.cpu_usage_percent.toFixed(1) }}%</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">实例</span><b
              class="node-detail-value code">{{ selected.instance_id || '-' }}</b></div>
          <div class="node-detail-field"><span class="node-detail-label">代次</span><b
              class="node-detail-value">{{ selected.generation }}</b></div>
          <div class="node-detail-field"><span class="node-detail-label">线程</span><b
              class="node-detail-value">{{ selected.host_metrics.process_threads }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">最后心跳</span><b
              class="node-detail-value">{{ formatDateTime(selected.last_seen_at_ms) }}</b></div>
          <div class="node-detail-field is-wide is-stacked"><span class="node-detail-label">Load（1m / 5m / 15m）</span><b
              class="node-detail-value">{{ selected.host_metrics.load_average_1m.toFixed(2) }} / {{
                selected.host_metrics.load_average_5m.toFixed(2) }} / {{ selected.host_metrics.load_average_15m.toFixed(2) }}</b></div>
          <div class="node-detail-field is-wide is-stacked"><span class="node-detail-label">能力</span><b
              class="node-detail-value">{{ selected.capabilities.join(', ') || '-' }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">内存</span><b
              class="node-detail-value">{{ formatBytes(selected.host_metrics.memory_used_bytes) }} / {{
                formatBytes(selected.host_metrics.memory_total_bytes) }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">磁盘 IO</span><b
              class="node-detail-value">↓{{ formatRate(selected.host_metrics.disk_read_bytes_per_sec) }} ↑{{
                formatRate(selected.host_metrics.disk_write_bytes_per_sec) }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">网络 IO</span><b
              class="node-detail-value">↓{{ formatRate(selected.host_metrics.network_receive_bytes_per_sec) }} ↑{{
                formatRate(selected.host_metrics.network_transmit_bytes_per_sec) }}</b></div>
          <div class="node-detail-field is-wide"><span class="node-detail-label">进程 RSS</span><b
              class="node-detail-value">{{ formatBytes(selected.host_metrics.process_resident_memory_bytes) }}</b></div>
          <div class="node-detail-field is-wide is-stacked"><span class="node-detail-label">业务指标</span><b
              class="node-detail-value">{{ businessMetrics }}</b></div>
        </div>
      </div>
      <template #footer><el-button @click="nodeDetailVisible = false">关闭</el-button></template>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { errorMessage, listLeases, listNodes, type LeaseInfo, type NodeInfo } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import StatusPill from '@/components/StatusPill.vue';
import { formatDateTime } from '@/utils/dateTime';

type AlertLevel = 'offline' | 'warning' | 'ready';
interface NodeTaskLoad {
  node: NodeInfo;
  running: number;
  queued: number;
  total: number;
  alertLevel: AlertLevel;
}

interface NodeFilters {
  kind: string;
  health: string;
  nodeId: string;
}

const PAGE_SIZE = 10;
const naturalCollator = new Intl.Collator('zh-CN', { numeric: true, sensitivity: 'base' });

const loading = ref(false);
const leases = ref<LeaseInfo[]>([]);
const nodes = ref<NodeInfo[]>([]);
const selected = ref<NodeInfo>();
const nodeDetailVisible = ref(false);
const activeTab = ref<'tasks' | 'matrix'>('tasks');
const taskPage = ref(1);
const matrixPage = ref(1);
const taskFilterDraft = reactive<NodeFilters>({ kind: '', health: '', nodeId: '' });
const taskFilters = reactive<NodeFilters>({ kind: '', health: '', nodeId: '' });
const matrixFilterDraft = reactive<NodeFilters>({ kind: '', health: '', nodeId: '' });
const matrixFilters = reactive<NodeFilters>({ kind: '', health: '', nodeId: '' });

function alertLevel(node: NodeInfo): AlertLevel {
  if (node.health === 'OFFLINE' || node.connection !== 'CONNECTED') return 'offline';
  if (node.health !== 'READY' || node.scheduling !== 'ENABLED') return 'warning';
  return 'ready';
}

function compareNodes(left: NodeInfo, right: NodeInfo) {
  const alertPriority = Number(alertLevel(left) === 'ready') - Number(alertLevel(right) === 'ready');
  return alertPriority
    || naturalCollator.compare(left.kind, right.kind)
    || naturalCollator.compare(left.node_id, right.node_id);
}

function matchesFilters(node: NodeInfo, filters: NodeFilters) {
  return (!filters.kind || node.kind === filters.kind)
    && (!filters.health || node.health === filters.health)
    && (!filters.nodeId || node.node_id === filters.nodeId);
}

const nodeLoads = computed<NodeTaskLoad[]>(() => {
  const counts = new Map<string, { running: number; queued: number }>();
  for (const lease of leases.value) {
    if (lease.state !== 'confirmed' && lease.state !== 'allocated') continue;
    const count = counts.get(lease.node_id) ?? { running: 0, queued: 0 };
    if (lease.state === 'confirmed') count.running += 1;
    else count.queued += 1;
    counts.set(lease.node_id, count);
  }
  return nodes.value
    .map((node) => {
      const count = counts.get(node.node_id) ?? { running: 0, queued: 0 };
      return { node, ...count, total: count.running + count.queued, alertLevel: alertLevel(node) };
    })
    .sort((left, right) => compareNodes(left.node, right.node));
});

const sortedNodes = computed(() => [...nodes.value].sort(compareNodes));
const kindOptions = computed(() => [...new Set(nodes.value.map((node) => node.kind))].sort(naturalCollator.compare));
const healthOptions = computed(() => [...new Map(nodes.value.map((node) => [node.health, node.health_label])).entries()]
  .map(([value, label]) => ({ value, label }))
  .sort((left, right) => naturalCollator.compare(left.label, right.label)));
const nodeIdOptions = computed(() => nodes.value.map((node) => node.node_id).sort(naturalCollator.compare));
const filteredTaskLoads = computed(() => nodeLoads.value.filter((item) => matchesFilters(item.node, taskFilters)));
const filteredMatrixNodes = computed(() => sortedNodes.value.filter((node) => matchesFilters(node, matrixFilters)));
const taskPageRows = computed(() => filteredTaskLoads.value.slice((taskPage.value - 1) * PAGE_SIZE, taskPage.value * PAGE_SIZE));
const matrixPageRows = computed(() => filteredMatrixNodes.value.slice((matrixPage.value - 1) * PAGE_SIZE, matrixPage.value * PAGE_SIZE));
const maxTaskCount = computed(() => Math.max(1, ...nodeLoads.value.map((item) => item.total)));
const readyCount = computed(() => nodeLoads.value.filter((item) => item.alertLevel === 'ready').length);
const warningCount = computed(() => nodeLoads.value.filter((item) => item.alertLevel === 'warning').length);
const offlineCount = computed(() => nodeLoads.value.filter((item) => item.alertLevel === 'offline').length);
const runningTaskCount = computed(() => nodeLoads.value.reduce((sum, item) => sum + item.running, 0));
const queuedTaskCount = computed(() => nodeLoads.value.reduce((sum, item) => sum + item.queued, 0));
const currentTaskCount = computed(() => runningTaskCount.value + queuedTaskCount.value);
const businessMetrics = computed(() => selected.value ? Object.entries(selected.value.business_metrics).map(([key, value]) => key + '=' + value).join(', ') || '-' : '-');
const nodeDetailTitle = computed(() => `实例围栏 · ${selected.value?.display_name || selected.value?.node_id || '-'}`);

function taskWidth(count: number) { return `${count / maxTaskCount.value * 100}%`; }
function memoryPercent(node: NodeInfo) { return node.host_metrics.memory_total_bytes ? Math.round(node.host_metrics.memory_used_bytes / node.host_metrics.memory_total_bytes * 100) : 0; }
function formatBytes(value?: number) { if (!value) return '0 B'; const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1); return (value / 1024 ** index).toFixed(index ? 1 : 0) + ' ' + units[index]; }
function formatRate(value: number) { return formatBytes(value) + '/s'; }
function openNodeDetail(node: NodeInfo) { selected.value = node; nodeDetailVisible.value = true; }
function queryTaskFilters() { Object.assign(taskFilters, taskFilterDraft); taskPage.value = 1; }
function resetTaskFilters() { Object.assign(taskFilterDraft, { kind: '', health: '', nodeId: '' }); queryTaskFilters(); }
function queryMatrixFilters() { Object.assign(matrixFilters, matrixFilterDraft); matrixPage.value = 1; }
function resetMatrixFilters() { Object.assign(matrixFilterDraft, { kind: '', health: '', nodeId: '' }); queryMatrixFilters(); }

async function load() {
  loading.value = true;
  try {
    const selectedNodeId = selected.value?.node_id;
    const [nextLeases, nextNodes] = await Promise.all([listLeases(), listNodes()]);
    leases.value = nextLeases;
    nodes.value = nextNodes;
    selected.value = nextNodes.find((node) => node.node_id === selectedNodeId) ?? nextNodes[0];
  } catch (error) {
    ElMessage.error(errorMessage(error, '系统健康数据加载失败'));
  } finally {
    loading.value = false;
  }
}

onMounted(() => { void load(); });
</script>

<style scoped>
.health-tabs-wrap {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
  position: relative;
}

.health-tabs-wrap > :deep(.el-tabs) {
  display: flex;
  flex: 1;
  flex-direction: column;
  min-height: 0;
}

.health-tabs-wrap :deep(.el-tabs__content) {
  flex: 1;
  min-height: 0;
}

.health-tabs-wrap :deep(.el-tab-pane) {
  display: flex;
  flex-direction: column;
  height: 100%;
  min-height: 0;
}

.health-refresh {
  position: absolute;
  z-index: 2;
  top: 0;
  right: 0;
}

.health-filter-scroll {
  margin-bottom: 16px;
  overflow-x: auto;
}

.health-filter-row {
  display: grid;
  grid-template-columns: repeat(3, minmax(180px, 1fr)) 180px;
  gap: 10px;
  min-width: 820px;
  width: 100%;
  box-sizing: border-box;
  padding: 0 16px 2px;
}

.health-filter-row :deep(.el-form-item) {
  margin: 0;
}

.health-filter-row :deep(.el-form-item__content),
.health-filter-row :deep(.el-select) {
  min-width: 0;
  width: 100%;
}

.health-filter-row :deep(.filter-actions) {
  width: 180px;
}

.health-filter-row :deep(.filter-actions .el-form-item__content) {
  display: grid;
  grid-template-columns: repeat(2, 1fr);
  gap: 10px;
}

.health-filter-row :deep(.filter-actions .el-button) {
  width: 100%;
  margin: 0;
}

.pager-row {
  display: flex;
  align-items: center;
  justify-content: flex-end;
  min-height: 48px;
}

.health-tab-body {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
}

.health-tab-body :deep(.el-empty) {
  height: 100%;
}

@media (max-width: 900px) {
  .health-tabs-wrap,
  .health-tabs-wrap > :deep(.el-tabs),
  .health-tabs-wrap :deep(.el-tab-pane) {
    display: block;
    height: auto;
  }

  .health-tab-body {
    height: 520px;
  }
}

.node-load-list {
  display: grid;
  gap: 6px;
  padding-right: 4px;
}

.node-load-row {
  display: grid;
  grid-template-columns: minmax(220px, 1fr) minmax(180px, .8fr) minmax(340px, 2fr);
  gap: 18px;
  align-items: center;
  min-height: 56px;
  padding: 14px 16px;
  border: 1px solid rgba(37, 146, 255, .32);
  border-radius: 14px;
  background: rgba(4, 16, 47, .58);
}

.node-load-row.is-ready {
  box-shadow: inset 3px 0 #35f0a1;
}

.node-load-row.is-warning {
  border-color: rgba(255, 209, 102, .72);
  background: linear-gradient(90deg, rgba(120, 78, 13, .32), rgba(42, 29, 19, .24));
  box-shadow: inset 4px 0 #ffd166;
}

.node-load-row.is-offline {
  border-color: rgba(255, 107, 138, .92);
  background: linear-gradient(90deg, rgba(137, 24, 56, .48), rgba(62, 9, 31, .32));
  box-shadow: inset 4px 0 #ff6b8a, 0 0 20px rgba(255, 70, 112, .12);
}

.node-load-identity {
  display: grid;
  min-width: 0;
  gap: 4px;
}

.node-load-identity b,
.node-load-identity span {
  overflow-wrap: anywhere;
}

.node-load-identity b {
  color: var(--text);
  font-size: 15px;
}

.node-load-identity span {
  color: #c8f3ff;
  font-size: 12px;
}

.node-load-identity small {
  color: var(--muted);
  font-size: 12px;
}

.node-load-state {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 9px;
}

.node-load-state>span:last-child {
  color: var(--muted);
  font-size: 12px;
  font-weight: 800;
}

.node-load-state>span.warning {
  color: var(--yellow);
}

.node-load-visual {
  display: grid;
  min-width: 0;
  gap: 6px;
}

.node-load-track {
  display: flex;
  width: 100%;
  height: 16px;
  overflow: hidden;
  border: 1px solid rgba(111, 176, 226, .34);
  border-radius: 999px;
  background: rgba(1, 10, 29, .72);
}

.node-load-track span {
  min-width: 3px;
  height: 100%;
}

.node-load-running {
  background: linear-gradient(90deg, #16b883, #35f0a1);
}

.node-load-queued {
  background: linear-gradient(90deg, #ffb84d, #ffd166);
}

.node-load-counts {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 14px;
  color: var(--muted);
  font-size: 12px;
}

.node-load-counts span::before {
  content: '';
  display: inline-block;
  width: 7px;
  height: 7px;
  margin-right: 6px;
  border-radius: 50%;
}

.node-load-counts .running::before {
  background: var(--green);
  box-shadow: 0 0 9px rgba(53, 240, 161, .62);
}

.node-load-counts .queued::before {
  background: var(--yellow);
  box-shadow: 0 0 9px rgba(255, 209, 102, .55);
}

.node-load-counts b {
  margin-left: auto;
  color: var(--text);
  font-size: 13px;
}

.node-matrix-table :deep(.el-table__row) {
  cursor: pointer;
}

.node-matrix-table :deep(.cell),
.io-cell {
  white-space: nowrap;
}

.node-detail {
  min-width: 0;
}

.node-detail-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.node-detail-field {
  display: grid;
  grid-template-columns: 52px minmax(0, 1fr);
  align-items: center;
  gap: 10px;
  min-width: 0;
  padding: 12px;
  border: 1px solid rgba(37, 146, 255, .22);
  border-radius: 14px;
  background: rgba(4, 16, 47, .48);
}

.node-detail-field.is-wide {
  grid-column: 1 / -1;
}

.node-detail-field.is-stacked {
  grid-template-columns: minmax(0, 1fr);
  align-items: start;
  gap: 6px;
}

.node-detail-label {
  color: var(--muted);
  font-size: 12px;
}

.node-detail-value {
  min-width: 0;
  margin: 0;
  justify-self: end;
  overflow-wrap: anywhere;
  color: var(--text);
  font-size: 15px;
  text-align: right;
}

.node-detail-field.is-stacked .node-detail-value {
  justify-self: start;
  text-align: left;
}

@media (max-width: 1100px) {
  .node-load-row {
    grid-template-columns: minmax(190px, 1fr) minmax(160px, .8fr) minmax(260px, 1.5fr);
    gap: 12px;
  }
}

@media (max-width: 900px) {
  .node-load-row {
    grid-template-columns: minmax(0, 1fr);
  }

  .node-load-counts b {
    margin-left: 0;
  }
}
</style>
