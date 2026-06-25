<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="READY 节点" value="16" trend="+2 / 10m" hint="健康" />
    <MetricCard class="span-3" label="DRAINING" value="1" trend="维护中" hint="可控" />
    <MetricCard class="span-3" label="时间偏差" value="1" trend="TIME_UNSYNCED" hint="需处理" />
    <MetricCard class="span-3" label="平均负载" value="58%" trend="容量稳定" hint="stream/avai" />
    <GlassPanel class="span-8" title="节点矩阵" subtitle="node_id / instance_id / generation 主动上报">
      <el-table :data="nodes" height="360">
        <el-table-column prop="node_id" label="节点 ID" width="150" />
        <el-table-column prop="service" label="服务" width="100" />
        <el-table-column label="状态" width="150">
          <template #default="{ row }"><StatusPill :label="row.status" :tone="row.status === 'TIME_UNSYNCED' ? 'warning' : row.status" /></template>
        </el-table-column>
        <el-table-column prop="instance_id" label="实例" width="120" />
        <el-table-column prop="generation" label="代次" width="80" />
        <el-table-column label="CPU" width="150"><template #default="{ row }"><el-progress :percentage="row.cpu" /></template></el-table-column>
        <el-table-column label="内存"><template #default="{ row }"><el-progress :percentage="row.memory" /></template></el-table-column>
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="实例围栏" subtitle="区分旧连接、旧心跳和旧实例任务">
      <div class="kv">
        <div class="kv-item"><span>当前实例</span><b class="code">018f-7a2</b></div>
        <div class="kv-item"><span>旧心跳</span><b>已拒绝</b></div>
        <div class="kv-item"><span>最大偏差</span><b>1280ms</b></div>
        <div class="kv-item"><span>策略</span><b>告警/拒入</b></div>
      </div>
      <OrbitChart :option="lineOption('健康评分', '#a875ff')" sm />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { lineOption, nodes } from '@/data/mock';
</script>
