<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="活跃流" :value="rows.filter((row) => row.state === 'RUNNING' || row.state === 'CONFIRMED').length" trend="SIM" hint="stream" />
    <MetricCard class="span-3" label="待确认租约" value="0" trend="PENDING" hint="stream" />
    <MetricCard class="span-3" label="孤儿流" value="0" trend="需对账" hint="ORPHAN" />
    <MetricCard class="span-3" label="RTP multi" value="预留" trend="feature flag" hint="后续" />
    <GlassPanel class="span-7" title="运行路由" subtitle="wormhole route map"><OrbitChart :option="graphOption" /></GlassPanel>
    <GlassPanel class="span-5" title="端点与租约" subtitle="服务确认实际资源">
      <el-table :data="rows" height="300">
        <el-table-column prop="stream_id" label="流 ID" width="130" />
        <el-table-column prop="channel_id" label="通道" width="90" />
        <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.state" :tone="row.state" /></template></el-table-column>
        <el-table-column prop="endpoint" label="端点" />
        <el-table-column label="操作" width="90"><template #default="{ row }"><el-button link type="danger" :disabled="row.state !== 'RUNNING'" @click="stop(row.stream_id)">停止</el-button></template></el-table-column>
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { listStreams, liveApi, stopStream } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { graphOption, streams } from '@/data/mock';

type StreamRow = { stream_id: string; channel_id: string; state: string; endpoint: string };
const rows = ref<StreamRow[]>(streams.map((item) => ({ ...item })));
async function refresh() {
  if (!liveApi) return;
  rows.value = (await listStreams()).map((item) => ({ ...item, state: item.state.toUpperCase() }));
}
async function stop(streamId: string) {
  if (liveApi) await stopStream(streamId);
  ElMessage.success('流已停止');
  await refresh();
}
onMounted(refresh);
</script>
