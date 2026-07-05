<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-7" title="调度星图" subtitle="节点容量与租约归属">
      <OrbitChart :option="topology" />
    </GlassPanel>
    <GlassPanel class="span-5" title="待处理队列压力" subtitle="pending leases / 100 队列上限">
      <OrbitChart :option="queuePressureRadar" />
    </GlassPanel>
    <GlassPanel class="span-12" title="租约生命周期" subtitle="ALLOCATED → CONFIRMED → RELEASED / EXPIRED">
      <el-table :data="leases" height="300" empty-text="暂无租约">
        <el-table-column prop="route_id" label="路由 ID" width="140" />
        <el-table-column prop="lease_id" label="租约 ID" width="140" />
        <el-table-column prop="resource_id" label="资源" width="140" />
        <el-table-column prop="node_id" label="节点" width="130" />
        <el-table-column label="状态" width="130"><template #default="{ row }"><StatusPill :label="row.state.toUpperCase()" :tone="row.state" /></template></el-table-column>
        <el-table-column label="到期时间"><template #default="{ row }">{{ new Date(row.expires_at_ms).toLocaleString('zh-CN') }}</template></el-table-column>
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { listLeases, listNodes, type LeaseInfo, type NodeInfo } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { graphOption, radarOption } from '@/data/charts';

const loading = ref(false);
const leases = ref<LeaseInfo[]>([]);
const nodes = ref<NodeInfo[]>([]);
const PENDING_LEASE_LIMIT = 100;

const topology = computed(() => graphOption([...nodes.value.map((node) => ({ name: node.node_id, value: 1 })), ...leases.value.map((lease) => ({ name: lease.lease_id, value: 1 }))], leases.value.map((lease) => ({ source: lease.node_id, target: lease.lease_id }))));
const queuePressureRadar = computed(() => radarOption(nodes.value.slice(0, 5).map((node) => node.node_id), nodes.value.slice(0, 5).map((node) => Math.min(100, Math.round(node.pending_leases / PENDING_LEASE_LIMIT * 100)))));

async function loadSystemState() {
  loading.value = true;
  try {
    [leases.value, nodes.value] = await Promise.all([listLeases(), listNodes()]);
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '调度数据加载失败');
  } finally {
    loading.value = false;
  }
}

onMounted(() => { void loadSystemState(); });
</script>
