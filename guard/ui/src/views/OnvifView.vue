<template>
  <div class="page-grid">
    <GlassPanel class="span-12" title="ONVIF" subtitle="ONVIF 设备接入占位">
      <el-empty description="ONVIF 控制台待接入">
        <template #description>
          <div class="placeholder-copy">
            当前仅开放菜单占位。后续 ONVIF 稳定后，将在这里提供设备发现、台账维护、通道映射与控制入口。
          </div>
        </template>
      </el-empty>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import GlassPanel from '@/components/GlassPanel.vue';
</script>

<style scoped>
.placeholder-copy {
  max-width: 620px;
  color: var(--muted);
  line-height: 1.8;
}
</style>
