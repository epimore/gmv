<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="MQTT" value="已连接" trend="QoS 1" hint="broker" />
    <MetricCard class="span-3" label="Webhook" value="4" trend="HMAC" hint="endpoint" />
    <MetricCard class="span-3" label="Outbox" value="27" trend="PENDING" hint="bounded" />
    <MetricCard class="span-3" label="Dead Letter" value="1" trend="需处理" hint="TTL" />
    <GlassPanel class="span-8" title="集成控制台" subtitle="MQTT / Webhook / Outbox">
      <el-table :data="integrations" height="330">
        <el-table-column prop="channel" label="通道" width="120" />
        <el-table-column prop="target" label="目标" />
        <el-table-column label="状态" width="140"><template #default="{ row }"><StatusPill :label="row.state" :tone="row.state" /></template></el-table-column>
        <el-table-column prop="retry" label="重试" width="90" />
        <el-table-column prop="security" label="安全" width="130" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="防护策略" subtitle="不执行脚本，受限字段映射">
      <div class="kv">
        <div class="kv-item"><span>HMAC</span><b>启用</b></div>
        <div class="kv-item"><span>SSRF</span><b>拦截</b></div>
        <div class="kv-item"><span>TTL</span><b>必填</b></div>
        <div class="kv-item"><span>幂等</span><b>command_id</b></div>
      </div>
      <OrbitChart :option="lineOption('投递成功率', '#35f0a1')" sm />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { integrations, lineOption } from '@/data/mock';
</script>
