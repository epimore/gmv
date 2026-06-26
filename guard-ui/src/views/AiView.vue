<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard class="span-3" label="任务记录" :value="rows.length" trend="Guard API" hint="avai" />
    <MetricCard class="span-3" label="运行任务" :value="runningCount" trend="RUNNING" hint="实时" />
    <MetricCard class="span-3" label="失败任务" :value="failedCount" trend="FAILED" hint="需处理" />
    <MetricCard class="span-3" label="可用流" :value="streamCount" trend="输入" hint="stream" />
    <GlassPanel class="span-5" title="任务状态" subtitle="真实任务分布"><OrbitChart :option="taskChart" /></GlassPanel>
    <GlassPanel class="span-7" title="任务队列" subtitle="guard 调度，avai 执行">
      <div class="toolbar"><el-button :loading="loading" @click="load">刷新</el-button><el-button type="primary" :disabled="!canOperate || !streamCount" @click="create">创建车辆分析</el-button></div>
      <el-alert v-if="unavailable" title="Guard 未启用 AI 业务适配器" type="warning" :closable="false" show-icon />
      <el-table :data="rows" height="250" empty-text="暂无 AI 任务">
        <el-table-column prop="task_id" label="任务 ID" width="140" /><el-table-column prop="model" label="模型" width="130" />
        <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.state.toUpperCase()" :tone="row.state" /></template></el-table-column>
        <el-table-column prop="node_id" label="avai 节点" /><el-table-column label="操作" width="90"><template #default="{ row }"><el-button link type="danger" :disabled="row.state !== 'running' || !canOperate" @click="cancel(row.task_id)">取消</el-button></template></el-table-column>
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'; import { ElMessage } from 'element-plus'; import { ApiError, cancelAiTask, listAiTasks, listStreams, startAiTask, type SimAiTask, type SimStream } from '@/api/client'; import GlassPanel from '@/components/GlassPanel.vue'; import MetricCard from '@/components/MetricCard.vue'; import OrbitChart from '@/components/OrbitChart.vue'; import StatusPill from '@/components/StatusPill.vue'; import { lineOption } from '@/data/charts'; import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const rows = ref<SimAiTask[]>([]); const streams = ref<SimStream[]>([]); const loading = ref(false); const unavailable = ref(false); const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const runningCount = computed(() => rows.value.filter((item) => item.state === 'running').length); const failedCount = computed(() => rows.value.filter((item) => item.state === 'failed').length); const streamCount = computed(() => streams.value.filter((item) => item.state === 'running').length); const taskChart = computed(() => lineOption('任务状态', [runningCount.value, rows.value.filter((item) => item.state === 'cancelled').length, failedCount.value], ['运行', '取消', '失败'], '#a875ff'));
async function load() { loading.value = true; unavailable.value = false; try { [rows.value, streams.value] = await Promise.all([listAiTasks(), listStreams()]); } catch (error) { if (error instanceof ApiError && error.status === 501) { unavailable.value = true; rows.value = []; streams.value = []; } else ElMessage.error(error instanceof Error ? error.message : 'AI 数据加载失败'); } finally { loading.value = false; } }
async function create() { const stream = streams.value.find((item) => item.state === 'running'); if (!stream) return; try { await startAiTask(stream.stream_id, 'vehicle', 'ui-ai-' + Date.now()); ElMessage.success('AI 任务已创建'); await load(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '创建失败'); } }
async function cancel(id: string) { try { await cancelAiTask(id); ElMessage.success('AI 任务已取消'); await load(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '取消失败'); } }
onMounted(load);
</script>
