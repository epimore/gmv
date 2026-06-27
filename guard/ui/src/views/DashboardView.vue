<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard v-for="item in metrics" :key="item.label" class="span-3" v-bind="item" />
    <GlassPanel class="span-7" title="星图拓扑" subtitle="Guard 实时资源关系">
      <OrbitChart :option="topology" />
    </GlassPanel>
    <GlassPanel class="span-5" title="资源分布" subtitle="节点、流、AI 与事件">
      <OrbitChart :option="resourceTrend" />
    </GlassPanel>
    <GlassPanel class="span-8" title="最近事件" subtitle="REST polling · after_id">
      <el-table :data="events" height="260" empty-text="暂无事件">
        <el-table-column prop="event_id" label="事件 ID" width="150" />
        <el-table-column prop="topic" label="主题" width="180" />
        <el-table-column prop="priority" label="优先级" width="90" />
        <el-table-column prop="message" label="内容" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="控制面状态" subtitle="API v2">
      <div class="kv">
        <div class="kv-item"><span>节点</span><b>{{ dashboard.node_count }}</b></div>
        <div class="kv-item"><span>事件</span><b>{{ dashboard.event_count }}</b></div>
        <div class="kv-item"><span>活跃流</span><b>{{ streams }}</b></div>
        <div class="kv-item"><span>AI 任务</span><b>{{ aiTasks }}</b></div>
        <div class="kv-item"><span>Cursor</span><b class="code">{{ dashboard.next_after_id || '-' }}</b></div>
      </div>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { ApiError, fetchDashboard, listAiTasks, listNodes, listStreams, pollEvents, type EventItem } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import { graphOption, lineOption } from '@/data/charts';

const loading = ref(false);
const dashboard = reactive({ node_count: 0, event_count: 0, next_after_id: null as string | null });
const nodeKinds = ref<Record<string, number>>({});
const streams = ref(0);
const aiTasks = ref(0);
const events = ref<Array<EventItem & { message: string }>>([]);
const metrics = computed(() => [
  { label: '在线节点', value: dashboard.node_count, trend: 'Guard registry', hint: '实时' },
  { label: '活跃流', value: streams.value, trend: 'stream', hint: '运行中' },
  { label: 'AI 任务', value: aiTasks.value, trend: 'avai', hint: '运行中' },
  { label: '事件', value: dashboard.event_count, trend: 'REST polling', hint: dashboard.next_after_id || '-' },
]);
const topology = computed(() => {
  const items = Object.entries(nodeKinds.value).map(([name, value]) => ({ name, value }));
  if (streams.value) items.push({ name: 'stream runtime', value: streams.value });
  if (aiTasks.value) items.push({ name: 'ai runtime', value: aiTasks.value });
  return graphOption(items, items.slice(1).map((item) => ({ source: items[0]?.name ?? item.name, target: item.name })));
});
const resourceTrend = computed(() => lineOption('资源数量', [dashboard.node_count, streams.value, aiTasks.value, dashboard.event_count], ['节点', '流', 'AI', '事件']));

function message(payload: string) {
  try { const value = JSON.parse(payload); return value.message ?? value.state ?? payload; } catch { return payload; }
}
async function optionalCount(loader: () => Promise<unknown[]>): Promise<number> {
  try { return (await loader()).length; } catch (error) { if (error instanceof ApiError && error.status === 501) return 0; throw error; }
}
async function load() {
  loading.value = true;
  try {
    const [summary, nodes, page, streamCount, taskCount] = await Promise.all([
      fetchDashboard(), listNodes(), pollEvents(undefined, 20), optionalCount(listStreams), optionalCount(listAiTasks),
    ]);
    Object.assign(dashboard, summary);
    nodeKinds.value = nodes.reduce((acc, node) => { acc[node.kind] = (acc[node.kind] ?? 0) + 1; return acc; }, {} as Record<string, number>);
    streams.value = streamCount;
    aiTasks.value = taskCount;
    events.value = page.items.map((item) => ({ ...item, message: message(item.payload) })).reverse();
  } catch (error) { ElMessage.error(error instanceof Error ? error.message : '总览加载失败'); }
  finally { loading.value = false; }
}
onMounted(load);
</script>
