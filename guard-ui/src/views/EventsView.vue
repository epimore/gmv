<template>
  <div class="page-grid">
    <GlassPanel class="span-8" title="事件中心" subtitle="REST cursor polling · after_id / next cursor">
      <div class="toolbar">
        <el-input style="width: 220px" placeholder="after_id" model-value="evt_000245" />
        <el-select style="width: 150px" model-value="全部级别"><el-option label="全部级别" value="全部级别" /><el-option label="P1" value="P1" /><el-option label="P2" value="P2" /></el-select>
        <el-button>暂停轮询</el-button>
        <el-button type="primary">继续拉取</el-button>
      </div>
      <el-table :data="events" height="360">
        <el-table-column prop="id" label="事件 ID" width="120" />
        <el-table-column prop="level" label="级别" width="80" />
        <el-table-column prop="type" label="类型" width="170" />
        <el-table-column prop="source" label="来源" width="130" />
        <el-table-column prop="message" label="说明" />
        <el-table-column prop="cursor" label="cursor" width="110" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="事件信封" subtitle="可复制 ID / 创建操作 / 标记已读">
      <div class="kv">
        <div class="kv-item"><span>after_id</span><b class="code">evt_000245</b></div>
        <div class="kv-item"><span>next cursor</span><b class="code">cur_7f91</b></div>
        <div class="kv-item"><span>P1</span><b>2 条</b></div>
        <div class="kv-item"><span>轮询</span><b>3s</b></div>
      </div>
      <div class="toolbar" style="margin-top: 16px;"><el-button>复制 ID</el-button><el-button type="primary">创建操作</el-button></div>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
import { events } from '@/data/mock';
</script>
