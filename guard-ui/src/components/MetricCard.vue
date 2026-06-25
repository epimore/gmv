<template>
  <GlassPanel class="metric-card">
    <div class="metric-value">{{ value }}</div>
    <div class="metric-label">{{ label }}</div>
    <div class="metric-trend">
      <span>{{ trend }}</span>
      <span>{{ hint }}</span>
    </div>
  </GlassPanel>
</template>

<script setup lang="ts">
import GlassPanel from './GlassPanel.vue';

defineProps<{ label: string; value: string | number; trend: string; hint: string }>();
</script>
