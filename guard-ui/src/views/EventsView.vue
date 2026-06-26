<template>
  <div class="page-grid" v-loading="loading">
    <GlassPanel class="span-8" title="事件中心" subtitle="REST cursor polling · after_id / next cursor">
      <div class="toolbar">
        <el-input v-model="afterId" style="width: 220px" placeholder="after_id（留空从头读取）" />
        <el-select v-model="minPriority" style="width: 150px"><el-option label="全部级别" :value="0" /><el-option label="P1+" :value="1" /><el-option label="P2+" :value="2" /><el-option label="P3+" :value="3" /></el-select>
        <el-input v-model="topicPrefix" style="width: 190px" placeholder="topic 前缀" />
        <el-button @click="paused = !paused">{{ paused ? '恢复轮询' : '暂停轮询' }}</el-button>
        <el-button type="primary" :disabled="paused" @click="load">拉取事件</el-button>
      </div>
      <el-table :data="rows" height="360" highlight-current-row empty-text="暂无事件" @current-change="selected = $event">
        <el-table-column prop="event_id" label="事件 ID" width="150" />
        <el-table-column prop="priorityLabel" label="级别" width="80" />
        <el-table-column prop="topic" label="主题" width="190" />
        <el-table-column prop="message" label="内容" />
      </el-table>
    </GlassPanel>
    <GlassPanel class="span-4" title="事件信封" subtitle="选中事件与操作入口">
      <div class="kv">
        <div class="kv-item"><span>event_id</span><b class="code">{{ selected?.event_id || '-' }}</b></div>
        <div class="kv-item"><span>next cursor</span><b class="code">{{ nextCursor || '-' }}</b></div>
        <div class="kv-item"><span>优先级</span><b>{{ selected?.priorityLabel || '-' }}</b></div>
        <div class="kv-item"><span>轮询</span><b>{{ paused ? '已暂停' : '手动/3s' }}</b></div>
      </div>
      <div class="toolbar" style="margin-top: 16px;">
        <el-button :disabled="!selected" @click="copyId">复制 ID</el-button>
        <el-button type="primary" :disabled="!selected || !canOperate" @click="createOperation">创建处理操作</el-button>
      </div>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { pollEvents, startOperation, type EventItem } from '@/api/client';
import GlassPanel from '@/components/GlassPanel.vue';
import { useAuthStore } from '@/stores/auth';
const auth = useAuthStore(); const loading = ref(false); const paused = ref(false); const afterId = ref(''); const nextCursor = ref(''); const minPriority = ref(0); const topicPrefix = ref('');
const rows = ref<Array<EventItem & { priorityLabel: string; message: string }>>([]); const selected = ref<(typeof rows.value)[number]>();
const canOperate = computed(() => auth.session?.role === 'operator' || auth.session?.role === 'admin');
let timer: number | undefined;
function message(payload: string) { try { const value = JSON.parse(payload); return value.message ?? value.state ?? payload; } catch { return payload; } }
async function load() { if (paused.value) return; loading.value = true; try { const page = await pollEvents(afterId.value || undefined, 100, minPriority.value || undefined, topicPrefix.value || undefined); rows.value = page.items.map((item) => ({ ...item, priorityLabel: 'P' + item.priority, message: message(item.payload) })).reverse(); nextCursor.value = page.next_after_id ?? ''; if (page.next_after_id) afterId.value = page.next_after_id; } catch (error) { ElMessage.error(error instanceof Error ? error.message : '事件加载失败'); } finally { loading.value = false; } }
async function copyId() { if (!selected.value) return; await navigator.clipboard.writeText(selected.value.event_id); ElMessage.success('事件 ID 已复制'); }
async function createOperation() { if (!selected.value) return; try { await startOperation('event.handle'); ElMessage.success('处理操作已提交'); } catch (error) { ElMessage.error(error instanceof Error ? error.message : '提交失败'); } }
onMounted(() => { void load(); timer = window.setInterval(() => { if (!paused.value) void load(); }, 3000); });
onBeforeUnmount(() => { if (timer) window.clearInterval(timer); });
</script>
