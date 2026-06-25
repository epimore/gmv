<template>
  <div class="page-grid">
    <MetricCard v-for="item in metrics" :key="item.label" class="span-3" v-bind="item" />
    <GlassPanel class="span-7" title="星图拓扑" subtitle="device/channel → session → stream → lease → avai">
      <OrbitChart :option="graphOption" />
    </GlassPanel>
    <GlassPanel class="span-5" title="运行趋势" subtitle="活跃流、任务和事件延迟">
      <OrbitChart :option="lineOption('活跃资源')" />
    </GlassPanel>
    <GlassPanel class="span-8" title="最近事件" subtitle="REST polling · after_id / next cursor">
      <el-table :data="events" height="260">
        <el-table-column prop="id" label="事件 ID" width="120" />
        <el-table-column prop="type" label="类型" width="170" />
        <el-table-column prop="source" label="来源" width="130" />
        <el-table-column prop="message" label="说明" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="控制面状态" subtitle="API v2">
      <div class="kv">
        <div class="kv-item"><span>SQLite</span><b>默认</b></div>
        <div class="kv-item"><span>TLS</span><b>启用</b></div>
        <div class="kv-item"><span>NTP</span><b>18ms</b></div>
        <div class="kv-item"><span>Cursor</span><b>cur_7f91</b></div>
      </div>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import { events, graphOption, lineOption, metrics } from '@/data/mock';
</script>
