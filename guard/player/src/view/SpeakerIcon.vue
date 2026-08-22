<template>
  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round"
    stroke-linejoin="round" aria-hidden="true">
    <path d="M11 5 6.5 9H3v6h3.5l4.5 4V5Z" />
    <template v-if="muted">
      <path d="m15 9 6 6" />
      <path d="m21 9-6 6" />
    </template>
    <template v-else>
      <path d="M15 8.5a5 5 0 0 1 0 7" />
      <path d="M18 5.5a9 9 0 0 1 0 13" />
    </template>
  </svg>
</template>

<script setup lang="ts">
withDefaults(defineProps<{ muted?: boolean }>(), { muted: false });
</script>
