<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard class="span-3" label="READY 节点" :value="readyCount" trend="健康" hint="registry" />
    <MetricCard class="span-3" label="DRAINING" :value="drainingCount" trend="维护中" hint="health" />
    <MetricCard class="span-3" label="时间异常" :value="unsyncedCount" trend="TIME_UNSYNCED" hint="调度禁用" />
    <MetricCard class="span-3" label="总容量" :value="totalCapacity" trend="capacity" hint="上报值" />
    <GlassPanel class="span-8" title="节点矩阵" subtitle="node_id / instance_id / generation 主动上报">
      <div class="toolbar"><el-button :loading="loading" @click="load">刷新</el-button></div>
      <el-table :data="nodes" height="360" highlight-current-row empty-text="暂无注册节点" @current-change="selected = $event">
        <el-table-column prop="node_id" label="节点 ID" width="150" />
        <el-table-column prop="kind" label="服务" width="100" />
        <el-table-column label="健康" width="120"><template #default="{ row }"><StatusPill :label="row.health" :tone="row.health" /></template></el-table-column>
        <el-table-column prop="scheduling" label="调度" width="130" />
        <el-table-column prop="instance_id" label="实例" min-width="160" />
        <el-table-column prop="generation" label="代次" width="80" />
        <el-table-column label="CPU" width="145"><template #default="{ row }"><el-progress :percentage="Math.round(row.host_metrics.cpu_usage_percent)" /></template></el-table-column>
        <el-table-column label="内存" width="145"><template #default="{ row }"><el-progress :percentage="memoryPercent(row)" /></template></el-table-column>
        <el-table-column label="Load" width="100"><template #default="{ row }">{{ row.host_metrics.load_average_1m.toFixed(2) }}</template></el-table-column>
        <el-table-column label="磁盘 IO" width="150"><template #default="{ row }">↓{{ formatRate(row.host_metrics.disk_read_bytes_per_sec) }} ↑{{ formatRate(row.host_metrics.disk_write_bytes_per_sec) }}</template></el-table-column>
        <el-table-column label="网络 IO" width="150"><template #default="{ row }">↓{{ formatRate(row.host_metrics.network_receive_bytes_per_sec) }} ↑{{ formatRate(row.host_metrics.network_transmit_bytes_per_sec) }}</template></el-table-column>
        <el-table-column prop="pending_leases" label="待确认租约" width="110" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="实例围栏" subtitle="当前选中节点的真实状态">
      <div class="kv">
        <div class="kv-item"><span>节点</span><b>{{ selected?.node_id || '-' }}</b></div>
        <div class="kv-item"><span>实例</span><b class="code">{{ selected?.instance_id || '-' }}</b></div>
        <div class="kv-item"><span>连接</span><b>{{ selected?.connection || '-' }}</b></div>
        <div class="kv-item"><span>最后心跳</span><b>{{ formatTime(selected?.last_seen_at_ms) }}</b></div>
        <div class="kv-item"><span>能力</span><b>{{ selected?.capabilities.join(', ') || '-' }}</b></div>
        <div class="kv-item"><span>内存</span><b>{{ formatBytes(selected?.host_metrics.memory_used_bytes) }} / {{ formatBytes(selected?.host_metrics.memory_total_bytes) }}</b></div>
        <div class="kv-item"><span>进程 RSS</span><b>{{ formatBytes(selected?.host_metrics.process_resident_memory_bytes) }}</b></div>
        <div class="kv-item"><span>线程</span><b>{{ selected?.host_metrics.process_threads ?? 0 }}</b></div>
        <div class="kv-item"><span>业务指标</span><b>{{ businessMetrics }}</b></div>
      </div>
      <OrbitChart :option="capacityChart" sm />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { listNodes, type NodeInfo } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { lineOption } from '@/data/charts';
const nodes = ref<NodeInfo[]>([]); const selected = ref<NodeInfo>(); const loading = ref(false);
const readyCount = computed(() => nodes.value.filter((item) => item.health === 'READY').length);
const drainingCount = computed(() => nodes.value.filter((item) => item.health === 'DRAINING').length);
const unsyncedCount = computed(() => nodes.value.filter((item) => item.scheduling === 'TIMEUNSYNCED' || item.scheduling === 'TIME_UNSYNCED').length);
const totalCapacity = computed(() => nodes.value.reduce((sum, item) => sum + item.capacity, 0));
const capacityChart = computed(() => lineOption('CPU 使用率', nodes.value.map((item) => item.host_metrics.cpu_usage_percent), nodes.value.map((item) => item.node_id), '#a875ff'));
const businessMetrics = computed(() => selected.value ? Object.entries(selected.value.business_metrics).map(([key, value]) => key + '=' + value).join(', ') || '-' : '-');
function formatTime(value?: number) { return value ? new Date(value).toLocaleString('zh-CN') : '-'; }
function memoryPercent(node: NodeInfo) { return node.host_metrics.memory_total_bytes ? Math.round(node.host_metrics.memory_used_bytes / node.host_metrics.memory_total_bytes * 100) : 0; }
function formatBytes(value?: number) { if (!value) return '0 B'; const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB']; const index = Math.min(Math.floor(Math.log(value) / Math.log(1024)), units.length - 1); return (value / 1024 ** index).toFixed(index ? 1 : 0) + ' ' + units[index]; }
function formatRate(value: number) { return formatBytes(value) + '/s'; }
async function load() { loading.value = true; try { nodes.value = await listNodes(); selected.value = nodes.value[0]; } catch (error) { ElMessage.error(error instanceof Error ? error.message : '节点加载失败'); } finally { loading.value = false; } }
onMounted(load);
</script>
