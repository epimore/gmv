<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard class="span-3" label="流记录" :value="rows.length" trend="Guard API" hint="stream" />
    <MetricCard class="span-3" label="运行中" :value="runningCount" trend="RUNNING" hint="实时" />
    <MetricCard class="span-3" label="失败" :value="failedCount" trend="FAILED" hint="需处理" />
    <MetricCard class="span-3" label="RTP multi" :value="multiCount" trend="endpoint" hint="多端口" />
    <GlassPanel class="span-7" title="运行路由" subtitle="stream → node → lease"><OrbitChart :option="topology" /></GlassPanel>
    <GlassPanel class="span-5" title="端点与租约" subtitle="服务确认实际资源">
      <div class="toolbar"><el-button :loading="loading" @click="load">刷新</el-button></div>
      <el-alert v-if="unavailable" title="Guard 未启用流媒体业务适配器" type="warning" :closable="false" show-icon />
      <el-table :data="rows" height="300" empty-text="暂无流">
        <el-table-column prop="stream_id" label="流 ID" width="140" />
        <el-table-column prop="channel_id" label="通道" width="100" />
        <el-table-column label="状态" width="110"><template #default="{ row }"><StatusPill :label="row.state.toUpperCase()" :tone="row.state" /></template></el-table-column>
        <el-table-column prop="endpoint" label="端点" />
        <el-table-column label="操作" width="90"><template #default="{ row }"><el-button link type="danger" :disabled="row.state !== 'running' || !canOperate" @click="stop(row.stream_id)">停止</el-button></template></el-table-column>
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'; import { ElMessage } from 'element-plus'; import { ApiError, listStreams, stopStream, type StreamSummary } from '@/api/client'; import GlassPanel from '@/components/GlassPanel.vue'; import MetricCard from '@/components/MetricCard.vue'; import OrbitChart from '@/components/OrbitChart.vue'; import StatusPill from '@/components/StatusPill.vue'; import { graphOption } from '@/data/charts'; import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const rows = ref<StreamSummary[]>([]); const loading = ref(false); const unavailable = ref(false); const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const runningCount = computed(() => rows.value.filter((item) => item.state === 'running').length); const failedCount = computed(() => rows.value.filter((item) => item.state === 'failed').length); const multiCount = computed(() => rows.value.filter((item) => item.endpoint.includes(',')).length);
const topology = computed(() => graphOption([...rows.value.map((item) => ({ name: item.stream_id, value: 1 })), ...Array.from(new Set(rows.value.map((item) => item.node_id))).map((name) => ({ name, value: 1 }))], rows.value.map((item) => ({ source: item.stream_id, target: item.node_id }))));
async function load() { loading.value = true; unavailable.value = false; try { rows.value = await listStreams(); } catch (error) { if (error instanceof ApiError && error.status === 501) { unavailable.value = true; rows.value = []; } else ElMessage.error(error instanceof Error ? error.message : '流加载失败'); } finally { loading.value = false; } }
async function stop(id: string) { try { await stopStream(id); ElMessage.success('流已停止'); await load(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '停止失败'); } }
onMounted(load);
</script>
