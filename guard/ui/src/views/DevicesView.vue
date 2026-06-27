<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-8" title="设备与通道" subtitle="Guard 设备目录与基础命令转发">
      <div class="toolbar">
        <el-input v-model="keyword" style="width: 260px" placeholder="搜索 device_id / 名称" />
        <el-button :loading="loading" @click="load">刷新目录</el-button>
        <el-button :disabled="!selected || !canOperate" @click="ptz">云台测试</el-button>
        <el-button type="primary" :disabled="!selected || !canOperate" @click="preview">发起预览</el-button>
      </div>
      <el-alert v-if="unavailable" title="Guard 未启用设备业务适配器" type="warning" :closable="false" show-icon />
      <el-table :data="filteredRows" height="360" highlight-current-row empty-text="暂无设备" @current-change="selected = $event">
        <el-table-column prop="device_id" label="设备 ID" width="210" />
        <el-table-column prop="name" label="名称" width="150" />
        <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.online ? 'ONLINE' : 'OFFLINE'" :tone="row.online ? 'ONLINE' : 'OFFLINE'" /></template></el-table-column>
        <el-table-column prop="session_node_id" label="所属 session" width="150" />
        <el-table-column label="通道"><template #default="{ row }">{{ row.channels.length }}</template></el-table-column>
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="设备详情" subtitle="Catalog Snapshot">
      <div class="kv">
        <div class="kv-item"><span>当前设备</span><b>{{ selected?.device_id ?? '请选择' }}</b></div>
        <div class="kv-item"><span>当前通道</span><b>{{ selected?.channels[0] ?? '-' }}</b></div>
        <div class="kv-item"><span>同步来源</span><b>{{ selected?.session_node_id ?? '-' }}</b></div>
        <div class="kv-item"><span>Guard 状态</span><b>{{ status?.guard_available === false ? '不可用' : '可用' }}</b></div>
        <div class="kv-item"><span>PTZ 命令</span><b>{{ status?.ptz_commands ?? 0 }}</b></div>
      </div>
      <OrbitChart :option="deviceChart" sm />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'; import { ElMessage } from 'element-plus';
import { ApiError, listDevices, sendPtz, runtimeStatus, startPreview, type DeviceSummary, type RuntimeStatus } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue'; import OrbitChart from '@/components/OrbitChart.vue'; import StatusPill from '@/components/StatusPill.vue'; import { lineOption } from '@/data/charts'; import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const rows = ref<DeviceSummary[]>([]); const selected = ref<DeviceSummary>(); const status = ref<RuntimeStatus>(); const loading = ref(false); const unavailable = ref(false); const keyword = ref('');
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
const filteredRows = computed(() => rows.value.filter((item) => !keyword.value || item.device_id.includes(keyword.value) || item.name.includes(keyword.value)));
const deviceChart = computed(() => lineOption('通道数', rows.value.map((item) => item.channels.length), rows.value.map((item) => item.name), '#35f0a1'));
async function load() { loading.value = true; unavailable.value = false; try { [rows.value, status.value] = await Promise.all([listDevices(), runtimeStatus()]); selected.value = rows.value[0]; } catch (error) { if (error instanceof ApiError && error.status === 501) { unavailable.value = true; rows.value = []; } else ElMessage.error(error instanceof Error ? error.message : '设备加载失败'); } finally { loading.value = false; } }
async function preview() { const item = selected.value; const channel = item?.channels[0]; if (!item || !channel) return; try { await startPreview(item.device_id, channel, 'ui-' + Date.now()); ElMessage.success('预览已创建'); status.value = await runtimeStatus(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '预览失败'); } }
async function ptz() { const item = selected.value; const channel = item?.channels[0]; if (!item || !channel) return; try { await sendPtz(item.device_id, channel); ElMessage.success('PTZ 命令已接受'); status.value = await runtimeStatus(); } catch (error) { ElMessage.error(error instanceof Error ? error.message : 'PTZ 失败'); } }
onMounted(load);
</script>
