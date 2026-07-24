<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard class="span-3" label="READY 节点" :value="readyCount" trend="可调度" :hint="`${nodes.length} 个节点`" />
    <MetricCard class="span-3" label="异常节点" :value="warningCount" trend="需关注" hint="健康或调度异常" />
    <MetricCard class="span-3" label="离线节点" :value="offlineCount" trend="连接中断" hint="OFFLINE" />
    <MetricCard class="span-3" label="当前任务" :value="currentTaskCount" :trend="`执行 ${runningTaskCount}`"
      :hint="`排队 ${queuedTaskCount}`" />

    <GlassPanel class="span-12" title="任务拥堵" subtitle="所有已知节点 · CONFIRMED 执行中 · ALLOCATED 排队中（已分配、待节点确认）">
      <template #action><el-button :loading="loading" @click="load">刷新</el-button></template>
      <div v-if="nodeLoads.length" class="node-load-list" role="list" aria-label="节点任务负载">
        <article v-for="item in nodeLoads" :key="item.node.node_id" class="node-load-row"
          :class="`is-${item.alertLevel}`" :data-node-id="item.node.node_id" :data-alert-level="item.alertLevel"
          role="listitem">
          <div class="node-load-identity">
            <!-- <b>{{ item.node.display_name || item.node.node_id }}</b> -->
            <b class="code">{{ item.node.node_id }}</b>
            <small>{{ item.node.kind }}</small>
          </div>
          <div class="node-load-state">
            <StatusPill :label="item.node.health" :tone="item.node.health" />
            <span :class="{ warning: item.node.scheduling !== 'ENABLED' }">{{ item.node.scheduling }}</span>
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
      <el-empty v-else description="暂无已知节点" />
    </GlassPanel>

    <GlassPanel class="span-12" title="节点矩阵" subtitle="点击节点查看实例围栏 · node_id / instance_id / generation 主动上报">
      <el-table class="node-matrix-table" :data="nodes" height="360" highlight-current-row empty-text="暂无注册节点"
        @row-click="openNodeDetail">
        <el-table-column prop="display_name" label="节点名称" width="180" />
        <el-table-column prop="node_id" label="节点 ID" width="150" />
        <el-table-column prop="kind" label="类型" width="90" />
        <!-- <el-table-column prop="service" label="服务" width="150" /> -->
        <el-table-column label="协议" width="110"><template #default="{ row }">{{ row.protocol || '-'
            }}</template></el-table-column>
        <el-table-column label="健康" width="120"><template #default="{ row }">
            <StatusPill :label="row.health" :tone="row.health" />
          </template></el-table-column>
        <el-table-column prop="scheduling" label="调度" width="130" />
        <el-table-column prop="instance_id" label="实例" min-width="160" />
        <el-table-column prop="generation" label="代次" width="80" />
        <el-table-column label="CPU" width="145"><template #default="{ row }"><el-progress
              :percentage="Math.round(row.host_metrics.cpu_usage_percent)" /></template></el-table-column>
        <el-table-column label="内存" width="145"><template #default="{ row }"><el-progress
              :percentage="memoryPercent(row)" /></template></el-table-column>
        <el-table-column label="Load" width="100"><template #default="{ row }">{{
          row.host_metrics.load_average_1m.toFixed(2) }}</template></el-table-column>
        <el-table-column label="磁盘 IO" width="150"><template #default="{ row }">↓{{
          formatRate(row.host_metrics.disk_read_bytes_per_sec) }} ↑{{
              formatRate(row.host_metrics.disk_write_bytes_per_sec) }}</template></el-table-column>
        <el-table-column label="网络 IO" width="150"><template #default="{ row }">↓{{
          formatRate(row.host_metrics.network_receive_bytes_per_sec) }} ↑{{
              formatRate(row.host_metrics.network_transmit_bytes_per_sec) }}</template></el-table-column>
      </el-table>
    </GlassPanel>

    <el-drawer v-model="nodeDetailVisible" :title="nodeDetailTitle" size="min(520px, 100vw)" destroy-on-close>
      <div v-if="selected" class="node-detail">
        <div class="kv">
          <div class="kv-item"><span>节点</span><b>{{ selected.display_name || selected.node_id }}</b></div>
          <div class="kv-item"><span>服务</span><b>{{ selected.service || '-' }}</b></div>
          <div class="kv-item"><span>协议</span><b>{{ selected.protocol || '-' }}</b></div>
          <div class="kv-item"><span>实例</span><b class="code">{{ selected.instance_id || '-' }}</b></div>
          <div class="kv-item"><span>连接</span><b>{{ selected.connection || '-' }}</b></div>
          <div class="kv-item"><span>最后心跳</span><b>{{ formatDateTime(selected.last_seen_at_ms) }}</b></div>
          <div class="kv-item"><span>能力</span><b>{{ selected.capabilities.join(', ') || '-' }}</b></div>
          <div class="kv-item"><span>内存</span><b>{{ formatBytes(selected.host_metrics.memory_used_bytes) }} / {{
            formatBytes(selected.host_metrics.memory_total_bytes) }}</b></div>
          <div class="kv-item"><span>进程 RSS</span><b>{{
            formatBytes(selected.host_metrics.process_resident_memory_bytes) }}</b></div>
          <div class="kv-item"><span>线程</span><b>{{ selected.host_metrics.process_threads }}</b></div>
          <div class="kv-item"><span>业务指标</span><b>{{ businessMetrics }}</b></div>
        </div>
        <OrbitChart :option="capacityChart" sm />
      </div>
      <template #footer><el-button @click="nodeDetailVisible = false">关闭</el-button></template>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { errorMessage, listLeases, listNodes, type LeaseInfo, type NodeInfo } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { lineOption } from '@/data/charts';
import { formatDateTime } from '@/utils/dateTime';

type AlertLevel = 'offline' | 'warning' | 'ready';
interface NodeTaskLoad {
  node: NodeInfo;
  running: number;
  queued: number;
  total: number;
  alertLevel: AlertLevel;
}

const loading = ref(false);
const leases = ref<LeaseInfo[]>([]);
const nodes = ref<NodeInfo[]>([]);
const selected = ref<NodeInfo>();
const nodeDetailVisible = ref(false);

function alertLevel(node: NodeInfo): AlertLevel {
  if (node.health === 'OFFLINE' || node.connection !== 'CONNECTED') return 'offline';
  if (node.health !== 'READY' || node.scheduling !== 'ENABLED') return 'warning';
  return 'ready';
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
  const priority: Record<AlertLevel, number> = { offline: 0, warning: 1, ready: 2 };
  return nodes.value
    .map((node) => {
      const count = counts.get(node.node_id) ?? { running: 0, queued: 0 };
      return { node, ...count, total: count.running + count.queued, alertLevel: alertLevel(node) };
    })
    .sort((left, right) => priority[left.alertLevel] - priority[right.alertLevel]
      || right.total - left.total
      || left.node.node_id.localeCompare(right.node.node_id));
});

const maxTaskCount = computed(() => Math.max(1, ...nodeLoads.value.map((item) => item.total)));
const readyCount = computed(() => nodeLoads.value.filter((item) => item.alertLevel === 'ready').length);
const warningCount = computed(() => nodeLoads.value.filter((item) => item.alertLevel === 'warning').length);
const offlineCount = computed(() => nodeLoads.value.filter((item) => item.alertLevel === 'offline').length);
const runningTaskCount = computed(() => nodeLoads.value.reduce((sum, item) => sum + item.running, 0));
const queuedTaskCount = computed(() => nodeLoads.value.reduce((sum, item) => sum + item.queued, 0));
const currentTaskCount = computed(() => runningTaskCount.value + queuedTaskCount.value);
const capacityChart = computed(() => lineOption('CPU 使用率', nodes.value.map((item) => item.host_metrics.cpu_usage_percent), nodes.value.map((item) => item.node_id), '#a875ff'));
const businessMetrics = computed(() => selected.value ? Object.entries(selected.value.business_metrics).map(([key, value]) => key + '=' + value).join(', ') || '-' : '-');
const nodeDetailTitle = computed(() => `实例围栏 · ${selected.value?.display_name || selected.value?.node_id || '-'}`);

function taskWidth(count: number) { return `${count / maxTaskCount.value * 100}%`; }
function memoryPercent(node: NodeInfo) { return node.host_metrics.memory_total_bytes ? Math.round(node.host_metrics.memory_used_bytes / node.host_metrics.memory_total_bytes * 100) : 0; }
function formatBytes(value?: number) { if (!value) return '0 B'; const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1); return (value / 1024 ** index).toFixed(index ? 1 : 0) + ' ' + units[index]; }
function formatRate(value: number) { return formatBytes(value) + '/s'; }
function openNodeDetail(node: NodeInfo) { selected.value = node; nodeDetailVisible.value = true; }

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
.node-load-list {
  display: grid;
  gap: 6px;
  max-height: 360px;
  overflow-y: auto;
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

.node-detail {
  display: grid;
  gap: 18px;
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
