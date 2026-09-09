<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard class="span-3" label="任务记录" :value="rows.length" trend="Guard API" hint="avai" />
    <MetricCard class="span-3" label="运行任务" :value="runningCount" trend="RUNNING" hint="实时" />
    <MetricCard class="span-3" label="失败任务" :value="failedCount" trend="FAILED" hint="需处理" />
    <MetricCard class="span-3" label="可用流" :value="streamCount" trend="输入" hint="stream" />
    <GlassPanel class="span-12" title="节点与模型交付" subtitle="Guard 按 installation_id + host_id 聚合 AVAI，并向同主机 gmv-center-agent 查询实时状态">
      <el-table :data="installations" empty-text="暂无 AVAI 或 gmv-center-agent 节点">
        <el-table-column label="安装实例" min-width="170">
          <template #default="{ row }">{{ row.installation_id || '未配置' }}</template>
        </el-table-column>
        <el-table-column prop="host_id" label="主机" min-width="150" />
        <el-table-column label="AVAI" min-width="190">
          <template #default="{ row }">{{ row.avai_nodes.map((node: AvaiInstallationNode) => node.node_id).join(', ') || '未部署' }}</template>
        </el-table-column>
        <el-table-column label="gmv-center-agent" min-width="180">
          <template #default="{ row }">{{ row.gmv_center_agent_nodes.map((node: AvaiInstallationNode) => node.node_id).join(', ') || '未部署' }}</template>
        </el-table-column>
        <el-table-column label="中心连接" width="135">
          <template #default="{ row }"><StatusPill :label="row.center_connection || 'UNAVAILABLE'" :tone="row.status_available && !row.stale ? 'running' : 'failed'" /></template>
        </el-table-column>
        <el-table-column label="当前 / 期望版本" min-width="190">
          <template #default="{ row }">{{ row.current_revision || '-' }} / {{ row.desired_revision || '-' }}</template>
        </el-table-column>
        <el-table-column label="交付状态" width="140">
          <template #default="{ row }">{{ row.delivery_state || row.status_error || '-' }}</template>
        </el-table-column>
      </el-table>
    </GlassPanel>
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
import { computed, onMounted, ref } from 'vue'; import { ElMessage } from 'element-plus'; import { ApiError, cancelAiTask, errorMessage, listAiTasks, listAvaiInstallations, listStreams, startAiTask, type AiTaskSummary, type AvaiInstallationInfo, type AvaiInstallationNode, type StreamSummary } from '@/api/client'; import GlassPanel from '@/components/GlassPanel.vue'; import MetricCard from '@/components/MetricCard.vue'; import OrbitChart from '@/components/OrbitChart.vue'; import StatusPill from '@/components/StatusPill.vue'; import { lineOption } from '@/data/charts'; import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const rows = ref<AiTaskSummary[]>([]); const streams = ref<StreamSummary[]>([]); const installations = ref<AvaiInstallationInfo[]>([]); const loading = ref(false); const unavailable = ref(false); const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const runningCount = computed(() => rows.value.filter((item) => item.state === 'running').length); const failedCount = computed(() => rows.value.filter((item) => item.state === 'failed').length); const streamCount = computed(() => streams.value.filter((item) => item.state === 'running').length); const taskChart = computed(() => lineOption('任务状态', [runningCount.value, rows.value.filter((item) => item.state === 'cancelled').length, failedCount.value], ['运行', '取消', '失败'], '#a875ff'));
async function load() { loading.value = true; unavailable.value = false; try { [rows.value, streams.value, installations.value] = await Promise.all([listAiTasks(), listStreams(), listAvaiInstallations()]); } catch (error) { if (error instanceof ApiError && error.status === 501) { unavailable.value = true; rows.value = []; streams.value = []; } else ElMessage.error(errorMessage(error, 'AI 数据加载失败')); } finally { loading.value = false; } }
async function create() { const stream = streams.value.find((item) => item.state === 'running'); if (!stream) return; try { await startAiTask(stream.stream_id, 'vehicle', 'ui-ai-' + Date.now()); ElMessage.success('AI 任务已创建'); await load(); } catch (error) { ElMessage.error(errorMessage(error, '创建失败')); } }
async function cancel(id: string) { try { await cancelAiTask(id); ElMessage.success('AI 任务已取消'); await load(); } catch (error) { ElMessage.error(errorMessage(error, '取消失败')); } }
onMounted(load);
</script>
