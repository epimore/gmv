<template>
  <div class="app-shell">
    <div class="star-arc" />
    <aside class="sidebar">
      <RouterLink class="brand" to="/dashboard" aria-label="GMV 总览">
        <div class="brand-mark">G</div>
        <div>
          <div class="brand-title">GMV</div>
          <div class="brand-sub">Control Plane</div>
        </div>
      </RouterLink>
      <nav>
        <template v-for="group in groups" :key="group">
          <div class="nav-group">{{ group }}</div>
          <RouterLink v-for="item in grouped[group]" :key="item.path" class="nav-item" :to="item.path">
            <span class="nav-icon">{{ item.icon }}</span>
            <span class="nav-label">{{ item.label }}</span>
          </RouterLink>
        </template>
      </nav>
      <div class="sidebar-footer">
        <b>REST polling</b>
        <span>after_id {{ polling.afterId }} · {{ polling.paused ? '已暂停' : '运行中' }}</span>
      </div>
    </aside>

    <main class="main">
      <header class="topbar">
        <div class="title">
          <h1>{{ route.meta.title }}</h1>
          <p>星舰控制面 · API v2</p>
        </div>
        <div class="top-actions">
          <div class="telemetry"><span class="dot" :class="{ paused: polling.paused }" />{{ polling.paused ? '轮询暂停' : 'REST 轮询' }}</div>
          <div class="telemetry">after_id <span class="code">{{ polling.afterId }}</span></div>
          <div class="telemetry">next cursor <span class="code">{{ polling.nextCursor }}</span></div>
          <el-button @click="polling.toggle()">{{ polling.paused ? '恢复' : '暂停' }}</el-button>
          <el-button type="primary" @click="polling.advance()">拉取增量</el-button>
        </div>
      </header>
      <RouterView />
    </main>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue';
import { useRoute } from 'vue-router';
import { menuRoutes } from '@/router';
import { usePollingStore } from '@/stores/polling';

const route = useRoute();
const polling = usePollingStore();
const groups = computed(() => [...new Set(menuRoutes.map((item) => item.group))]);
const grouped = computed(() =>
  menuRoutes.reduce(
    (acc, item) => {
      (acc[item.group] ||= []).push(item);
      return acc;
    },
    {} as Record<string, Array<(typeof menuRoutes)[number]>>,
  ),
);
</script>
