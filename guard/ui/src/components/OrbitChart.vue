<template>
  <div ref="el" class="chart" :class="{ sm }" />
</template>

<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, watch } from 'vue';
import { useResizeObserver } from '@vueuse/core';
import * as echarts from 'echarts';

const props = defineProps<{ option: Record<string, unknown>; sm?: boolean }>();
const el = ref<HTMLDivElement>();
let chart: echarts.ECharts | undefined;

const render = () => {
  if (!el.value) return;
  chart ||= echarts.init(el.value, 'dark');
  chart.setOption(props.option, true);
};

onMounted(render);
watch(() => props.option, render, { deep: true });
useResizeObserver(el, () => chart?.resize());
onBeforeUnmount(() => chart?.dispose());
</script>
