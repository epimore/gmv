<template>
  <div class="page-grid" v-loading="loading">
    <MetricCard class="span-3" label="MQTT 投递" :value="mqttCount" trend="outbox" hint="记录" />
    <MetricCard class="span-3" label="Webhook 投递" :value="webhookCount" trend="HMAC" hint="记录" />
    <MetricCard class="span-3" label="待投递" :value="pendingCount" trend="PENDING" hint="bounded" />
    <MetricCard class="span-3" label="Dead Letter" :value="deadCount" trend="需处理" hint="人工重试" />
    <GlassPanel class="span-8" title="集成控制台" subtitle="MQTT / Webhook / Outbox">
      <div class="toolbar"><el-button :loading="loading" @click="load">刷新</el-button></div>
      <el-table :data="records" height="330" empty-text="暂无 Outbox 记录">
        <el-table-column prop="destination_kind" label="通道" width="110" />
        <el-table-column prop="destination" label="目标" />
        <el-table-column label="状态" width="130"><template #default="{ row }"><StatusPill :label="row.state.toUpperCase()" :tone="row.state" /></template></el-table-column>
        <el-table-column prop="attempts" label="重试" width="80" />
        <el-table-column prop="last_error" label="最近错误" min-width="150" />
        <el-table-column label="操作" width="100"><template #default="{ row }"><el-button link type="primary" :disabled="row.state !== 'dead' || !canOperate" @click="retry(row.outbox_id)">重试</el-button></template></el-table-column>
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="投递状态" subtitle="真实 Outbox 状态分布">
      <OrbitChart :option="stateChart" sm />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'; import { ElMessage } from 'element-plus';
import { listOutbox, retryOutbox, type OutboxInfo } from '@/api/client'; import GlassPanel from '@/components/GlassPanel.vue'; import MetricCard from '@/components/MetricCard.vue'; import OrbitChart from '@/components/OrbitChart.vue'; import StatusPill from '@/components/StatusPill.vue'; import { lineOption } from '@/data/charts'; import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const records = ref<OutboxInfo[]>([]); const loading = ref(false); const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const mqttCount = computed(() => records.value.filter((item) => item.destination_kind === 'mqtt').length); const webhookCount = computed(() => records.value.filter((item) => item.destination_kind === 'webhook').length); const pendingCount = computed(() => records.value.filter((item) => item.state === 'pending' || item.state === 'retry_wait' || item.state === 'sending').length); const deadCount = computed(() => records.value.filter((item) => item.state === 'dead').length);
const stateChart = computed(() => lineOption('投递状态', [pendingCount.value, records.value.filter((item) => item.state === 'delivered').length, deadCount.value], ['待投递', '已投递', '死信'], '#35f0a1'));
async function load() { loading.value = true; try { records.value = await listOutbox(200); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '集成数据加载失败'); } finally { loading.value = false; } }
async function retry(id: string) { try { await retryOutbox(id); ElMessage.success('已重新进入投递队列'); await load(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '重试失败'); } }
onMounted(load);
</script>
