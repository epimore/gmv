<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="运行任务" :value="rows.filter((row) => row.state === 'RUNNING').length" trend="RUNNING" hint="avai" />
    <MetricCard class="span-3" label="队列深度" value="0" trend="可调度" hint="PENDING" />
    <MetricCard class="span-3" label="GPU 平均" value="68%" trend="容量健康" hint="sim" />
    <MetricCard class="span-3" label="FPS" value="24" trend="SIM" hint="吞吐" />
    <GlassPanel class="span-5" title="模型容量" subtitle="GPU / CPU / FPS"><OrbitChart :option="radarOption" /></GlassPanel>
    <GlassPanel class="span-7" title="任务队列" subtitle="guard 调度，avai 执行">
      <div class="toolbar"><el-button type="primary" :disabled="!liveApi" @click="create">创建车辆分析</el-button></div>
      <el-table :data="rows" height="250">
        <el-table-column prop="task_id" label="任务 ID" width="130" />
        <el-table-column prop="model" label="模型" width="130" />
        <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.state" :tone="row.state" /></template></el-table-column>
        <el-table-column prop="node" label="avai 节点" />
        <el-table-column label="操作" width="90"><template #default="{ row }"><el-button link type="danger" :disabled="row.state !== 'RUNNING'" @click="cancel(row.task_id)">取消</el-button></template></el-table-column>
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { cancelAiTask, listAiTasks, listStreams, liveApi, startAiTask } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { aiTasks, radarOption } from '@/data/mock';

type TaskRow = { task_id: string; model: string; state: string; node: string };
const rows = ref<TaskRow[]>(aiTasks.map((item) => ({ ...item })));
async function refresh() {
  if (!liveApi) return;
  rows.value = (await listAiTasks()).map((item) => ({ task_id: item.task_id, model: item.model, state: item.state.toUpperCase(), node: item.node_id }));
}
async function create() {
  const streams = await listStreams();
  const running = streams.find((item) => item.state === 'running');
  if (!running) { ElMessage.warning('请先创建预览流'); return; }
  await startAiTask(running.stream_id, 'vehicle', `ui-ai-${Date.now()}`);
  ElMessage.success('AI 任务已创建');
  await refresh();
}
async function cancel(taskId: string) {
  await cancelAiTask(taskId);
  ElMessage.success('AI 任务已取消');
  await refresh();
}
onMounted(refresh);
</script>
