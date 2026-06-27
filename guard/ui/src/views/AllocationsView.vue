<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-7" title="调度星图" subtitle="节点容量与租约归属">
      <OrbitChart :option="topology" />
    </GlassPanel>
    <GlassPanel class="span-5" title="候选容量" subtitle="节点上报 capacity / pending leases">
      <OrbitChart :option="capacityRadar" />
    </GlassPanel>
    <GlassPanel class="span-8" title="租约生命周期" subtitle="ALLOCATED → CONFIRMED → RELEASED / EXPIRED">
      <div class="toolbar">
        <el-button :loading="loading" @click="load">刷新</el-button>
        <el-button type="primary" :disabled="!canOperate" @click="rebalance">提交重新均衡</el-button>
      </div>
      <el-table :data="leases" height="300" empty-text="暂无租约">
        <el-table-column prop="route_id" label="路由 ID" width="140" />
        <el-table-column prop="lease_id" label="租约 ID" width="140" />
        <el-table-column prop="resource_id" label="资源" width="140" />
        <el-table-column prop="node_id" label="节点" width="130" />
        <el-table-column label="状态" width="130"><template #default="{ row }"><StatusPill :label="row.state.toUpperCase()" :tone="row.state" /></template></el-table-column>
        <el-table-column label="到期时间"><template #default="{ row }">{{ new Date(row.expires_at_ms).toLocaleString('zh-CN') }}</template></el-table-column>
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="控制操作" subtitle="最近提交的调度操作">
      <el-table :data="operations" height="300" empty-text="暂无操作">
        <el-table-column prop="kind" label="类型" />
        <el-table-column prop="status" label="状态" width="110" />
        <el-table-column prop="requested_by" label="发起人" width="100" />
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { listLeases, listNodes, listOperations, startOperation, type LeaseInfo, type NodeInfo, type OperationInfo } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue'; import OrbitChart from '@/components/OrbitChart.vue'; import StatusPill from '@/components/StatusPill.vue';
import { graphOption, radarOption } from '@/data/charts'; import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const loading = ref(false); const leases = ref<LeaseInfo[]>([]); const nodes = ref<NodeInfo[]>([]); const operations = ref<OperationInfo[]>([]);
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const topology = computed(() => graphOption([...nodes.value.map((node) => ({ name: node.node_id, value: node.capacity })), ...leases.value.map((lease) => ({ name: lease.lease_id, value: 1 }))], leases.value.map((lease) => ({ source: lease.node_id, target: lease.lease_id }))));
const capacityRadar = computed(() => radarOption(nodes.value.slice(0, 5).map((node) => node.node_id), nodes.value.slice(0, 5).map((node) => node.capacity ? Math.max(0, Math.min(100, 100 - Math.round(node.pending_leases / node.capacity * 100))) : 0)));
async function load() { loading.value = true; try { [leases.value, nodes.value, operations.value] = await Promise.all([listLeases(), listNodes(), listOperations()]); operations.value = operations.value.slice().reverse(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '调度数据加载失败'); } finally { loading.value = false; } }
async function rebalance() { try { await startOperation('scheduler.rebalance'); ElMessage.success('重新均衡操作已提交'); await load(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '提交失败'); } }
onMounted(load);
</script>
