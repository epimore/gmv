<template>
  <div class="page-grid">
    <GlassPanel class="span-7" title="调度星图" subtitle="候选评分、容量比例与稳定排序">
      <OrbitChart :option="graphOption" />
    </GlassPanel>
    <GlassPanel class="span-5" title="候选评分" subtitle="能力硬过滤后进入权重评分">
      <OrbitChart :option="radarOption" />
    </GlassPanel>
    <GlassPanel class="span-12" title="租约生命周期" subtitle="PENDING → CONFIRMED → RELEASED / EXPIRED">
      <div class="toolbar">
        <el-button>确认租约</el-button>
        <el-button>释放租约</el-button>
        <el-button type="primary">重新均衡</el-button>
      </div>
      <el-table :data="leases" height="300">
        <el-table-column prop="allocation_id" label="调度 ID" width="130" />
        <el-table-column prop="lease_id" label="租约 ID" width="120" />
        <el-table-column prop="resource" label="资源" width="120" />
        <el-table-column prop="type" label="类型" width="110" />
        <el-table-column label="状态" width="130"><template #default="{ row }"><StatusPill :label="row.state" :tone="row.state" /></template></el-table-column>
        <el-table-column label="评分"><template #default="{ row }"><el-progress :percentage="Math.round(row.score * 100)" /></template></el-table-column>
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { graphOption, leases, radarOption } from '@/data/mock';
</script>
