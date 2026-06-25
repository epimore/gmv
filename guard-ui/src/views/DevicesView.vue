<template>
  <div class="page-grid">
    <GlassPanel class="span-8" title="设备与通道" subtitle="首版仅做可观测与基础命令转发，不维护设备主数据">
      <div class="toolbar">
        <el-input style="width: 260px" placeholder="搜索 device_id / 名称" />
        <el-button :loading="loading" @click="refresh">刷新目录</el-button>
        <el-button :disabled="!selected" @click="ptz">云台测试</el-button>
        <el-button type="primary" :disabled="!selected" @click="preview">发起预览</el-button>
      </div>
      <el-table :data="rows" height="360" highlight-current-row @current-change="select">
        <el-table-column prop="device_id" label="设备 ID" width="210" />
        <el-table-column prop="name" label="名称" width="130" />
        <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.status" :tone="row.status" /></template></el-table-column>
        <el-table-column prop="session" label="所属 session" width="140" />
        <el-table-column prop="channels" label="通道" width="80" />
        <el-table-column prop="active" label="活跃流" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="设备详情" subtitle="Catalog Snapshot">
      <div class="kv">
        <div class="kv-item"><span>当前设备</span><b>{{ selected?.device_id ?? '请选择' }}</b></div>
        <div class="kv-item"><span>当前通道</span><b>{{ selected?.channel_id ?? '-' }}</b></div>
        <div class="kv-item"><span>同步来源</span><b>session</b></div>
        <div class="kv-item"><span>模式</span><b>{{ liveApi ? 'SIM LIVE' : 'MOCK' }}</b></div>
      </div>
      <OrbitChart :option="lineOption('目录快照', '#35f0a1')" sm />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { listDevices, liveApi, sendPtz, startPreview } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { devices, lineOption } from '@/data/mock';

type DeviceRow = { device_id: string; name: string; status: string; session: string; channels: number; active: number; channel_id: string };
const rows = ref<DeviceRow[]>(devices.map((item) => ({ ...item, channel_id: 'ch-1' })));
const selected = ref<DeviceRow>();
const loading = ref(false);

function select(row?: DeviceRow) { selected.value = row; }
async function refresh() {
  if (!liveApi) return;
  loading.value = true;
  try {
    rows.value = (await listDevices()).map((item) => ({
      device_id: item.device_id, name: item.name, status: item.online ? 'ONLINE' : 'OFFLINE',
      session: item.session_node_id, channels: item.channels.length, active: 0, channel_id: item.channels[0] ?? '',
    }));
    selected.value = rows.value[0];
  } finally { loading.value = false; }
}
async function preview() {
  if (!selected.value) return;
  if (liveApi) await startPreview(selected.value.device_id, selected.value.channel_id, `ui-${Date.now()}`);
  ElMessage.success('预览已创建');
}
async function ptz() {
  if (!selected.value) return;
  if (liveApi) await sendPtz(selected.value.device_id, selected.value.channel_id);
  ElMessage.success('PTZ 命令已接受');
}
onMounted(refresh);
</script>
