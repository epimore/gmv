<template>
  <div class="page-grid">
    <GlassPanel class="span-5" title="系统健康" subtitle="TLS 默认、NTP/chrony、存储后端">
      <div class="kv">
        <div class="kv-item"><span>展示品牌</span><b>GMV</b></div>
        <div class="kv-item"><span>后端服务</span><b>guard</b></div>
        <div class="kv-item"><span>存储</span><b>SQLite 默认</b></div>
        <div class="kv-item"><span>TLS</span><b>默认启用</b></div>
        <div class="kv-item"><span>NTP 偏差</span><b>18ms</b></div>
        <div class="kv-item"><span>API</span><b>v2</b></div>
      </div>
    </GlassPanel>
    <GlassPanel class="span-7" title="系统任务" subtitle="备份、恢复、迁移、证书和审计">
      <el-table :data="systemJobs" height="300">
        <el-table-column prop="job" label="任务" width="150" />
        <el-table-column label="状态" width="120"><template #default="{ row }"><StatusPill :label="row.state" :tone="row.state === 'WARNING' ? 'warning' : row.state" /></template></el-table-column>
        <el-table-column label="进度" width="180"><template #default="{ row }"><el-progress :percentage="row.progress" /></template></el-table-column>
        <el-table-column prop="detail" label="说明" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-12" title="安全操作" subtitle="危险动作需二次确认与审计">
      <div class="toolbar">
        <el-button>开始备份</el-button>
        <el-button>恢复校验</el-button>
        <el-button>轮转证书</el-button>
        <el-button type="primary">导出配置</el-button>
      </div>
      <OrbitChart :option="lineOption('系统事件', '#a875ff')" />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { lineOption, systemJobs } from '@/data/mock';
</script>
