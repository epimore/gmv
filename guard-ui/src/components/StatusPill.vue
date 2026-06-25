<template>
  <span class="status-pill" :class="statusClass">{{ label }}</span>
</template>

<script setup lang="ts">
import { computed } from 'vue';

const props = defineProps<{ label: string; tone?: string }>();
const statusClass = computed(() => `status-${(props.tone || props.label).toLowerCase()}`);
</script>
