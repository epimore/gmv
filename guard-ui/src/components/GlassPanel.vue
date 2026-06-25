<template>
  <section class="glass-panel">
    <div class="panel-inner">
      <div v-if="title || subtitle" class="panel-head">
        <div>
          <h2 class="panel-title">{{ title }}</h2>
          <div v-if="subtitle" class="panel-kicker">{{ subtitle }}</div>
        </div>
        <slot name="action" />
      </div>
      <slot />
    </div>
  </section>
</template>

<script setup lang="ts">
defineProps<{ title?: string; subtitle?: string }>();
</script>
