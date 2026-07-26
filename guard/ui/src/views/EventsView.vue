<template>
  <div class="page-grid events-page">
    <GlassPanel
      class="span-12 event-console"
      title="告警与事件"
      subtitle="有界事件时间线 · REST 增量获取 · 关联诊断入口"
    >
      <template #action>
        <div class="console-state">
          <StatusPill
            :label="paused ? '已暂停' : '正在接收'"
            :tone="paused ? 'warning' : 'ready'"
          />
          <span>{{ lastFetchLabel }}</span>
        </div>
      </template>

      <div class="event-summary" aria-label="事件摘要">
        <div>
          <span>当前列表</span><b>{{ rows.length }}</b
          ><small>最多保留 {{ MAX_ROWS }} 条</small>
        </div>
        <div class="is-danger">
          <span>P0 紧急</span><b>{{ priorityCount(0) }}</b
          ><small>需要立即处理</small>
        </div>
        <div class="is-warning">
          <span>P1 告警</span><b>{{ priorityCount(1) }}</b
          ><small>需要关注</small>
        </div>
        <div>
          <span>待合并</span><b>{{ pendingRows.length }}</b
          ><small>{{ paused ? "恢复后显示" : "实时合并" }}</small>
        </div>
      </div>

      <div class="event-toolbar">
        <el-select v-model="priorityFilter" class="filter-priority" aria-label="事件级别">
          <el-option label="全部级别" value="all" />
          <el-option label="P0 紧急" value="0" />
          <el-option label="P1 告警" value="1" />
          <el-option label="P2 状态" value="2" />
          <el-option label="P3 遥测" value="3" />
        </el-select>
        <el-select v-model="domainFilter" class="filter-domain" aria-label="事件领域">
          <el-option label="全部领域" value="all" />
          <el-option
            v-for="domain in domainOptions"
            :key="domain.value"
            :label="domain.label"
            :value="domain.value"
          />
        </el-select>
        <el-input
          v-model="keyword"
          class="filter-keyword"
          clearable
          placeholder="搜索事件、资源、节点或摘要"
          aria-label="事件搜索"
        />
        <el-button @click="togglePaused">{{
          paused ? `恢复接收${pendingRows.length ? `（${pendingRows.length}）` : ""}` : "暂停接收"
        }}</el-button>
        <el-button type="primary" :loading="loading" @click="poll">{{
          paused ? "检查新事件" : "立即刷新"
        }}</el-button>
      </div>

      <div v-if="routeEventId" class="route-filter">
        <span>来自 Dashboard：{{ routeEventId }}</span>
        <el-button link @click="clearRouteEvent">清除定位</el-button>
      </div>

      <el-table
        v-loading="loading && !rows.length"
        :data="filteredRows"
        height="460"
        highlight-current-row
        :empty-text="emptyText"
        @row-click="openDetail"
      >
        <el-table-column prop="fetchedAtLabel" label="获取时间" width="170" />
        <el-table-column label="级别" width="105">
          <template #default="{ row }"
            ><StatusPill :label="row.priorityLabel" :tone="row.priorityTone"
          /></template>
        </el-table-column>
        <el-table-column prop="domainLabel" label="领域" width="120" />
        <el-table-column prop="eventType" label="事件类型" min-width="220" show-overflow-tooltip />
        <el-table-column prop="resourceId" label="资源对象" min-width="170" show-overflow-tooltip />
        <el-table-column prop="message" label="摘要" min-width="300" show-overflow-tooltip />
      </el-table>
      <div class="console-foot">
        <span>当前仅展示 Guard 已返回事件；获取时间不是事件发生时间。</span>
        <span>{{ filteredRows.length }} / {{ rows.length }} 条</span>
      </div>
    </GlassPanel>

    <el-drawer v-model="detailVisible" title="事件详情" size="min(640px, 100vw)" destroy-on-close>
      <template v-if="selected">
        <div class="event-detail-head">
          <div>
            <small>{{ selected.domainLabel }}</small>
            <h3>{{ selected.eventType }}</h3>
            <p>{{ selected.message }}</p>
          </div>
          <StatusPill :label="selected.priorityLabel" :tone="selected.priorityTone" />
        </div>
        <div class="kv event-detail-kv">
          <div class="kv-item wide">
            <span>Event ID</span><b class="code">{{ selected.event_id }}</b>
          </div>
          <div class="kv-item wide">
            <span>Topic</span><b class="code">{{ selected.topic }}</b>
          </div>
          <div class="kv-item">
            <span>获取时间</span><b>{{ selected.fetchedAtLabel }}</b>
          </div>
          <div class="kv-item">
            <span>发生时间</span><b>{{ selected.occurredAtLabel }}</b>
          </div>
          <div class="kv-item">
            <span>来源节点</span><b class="code">{{ selected.sourceNode }}</b>
          </div>
          <div class="kv-item">
            <span>资源对象</span><b class="code">{{ selected.resourceId }}</b>
          </div>
          <div class="kv-item">
            <span>Operation ID</span><b class="code">{{ selected.operationId }}</b>
          </div>
          <div class="kv-item">
            <span>Correlation ID</span><b class="code">{{ selected.correlationId }}</b>
          </div>
        </div>
        <section class="payload-section">
          <div class="payload-head">
            <h4>原始 Payload</h4>
            <el-button link @click="copyPayload">复制 Payload</el-button>
          </div>
          <pre>{{ selected.rawPayload }}</pre>
        </section>
      </template>
      <template #footer>
        <el-button :disabled="!selected" @click="copyEventId">复制 Event ID</el-button>
        <el-button type="primary" @click="detailVisible = false">关闭</el-button>
      </template>
    </el-drawer>
  </div>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useRoute, useRouter } from "vue-router";
import { errorMessage, pollEvents, type EventItem } from "@/api/client";
import GlassPanel from "@/components/GlassPanel.vue";
import StatusPill from "@/components/StatusPill.vue";
import { formatDateTime } from "@/utils/dateTime";

type Tone = "ready" | "warning" | "danger" | "info";
interface EventRow extends EventItem {
  fetchedAt: number;
  fetchedAtLabel: string;
  occurredAtLabel: string;
  priorityLabel: string;
  priorityTone: Tone;
  domain: string;
  domainLabel: string;
  eventType: string;
  resourceId: string;
  sourceNode: string;
  operationId: string;
  correlationId: string;
  message: string;
  rawPayload: string;
}

const EVENT_POLLING_INTERVAL_MS = 5_000;
const EVENT_BACKGROUND_POLLING_INTERVAL_MS = 30_000;
const MAX_ROWS = 300;
const route = useRoute();
const router = useRouter();
const loading = ref(false);
const paused = ref(false);
const cursor = ref("");
const lastFetchAt = ref(0);
const rows = ref<EventRow[]>([]);
const pendingRows = ref<EventRow[]>([]);
const selected = ref<EventRow>();
const detailVisible = ref(false);
const priorityFilter = ref("all");
const domainFilter = ref("all");
const keyword = ref("");
let timer: number | undefined;

const routeEventId = computed(() =>
  typeof route.query.event_id === "string" ? route.query.event_id : "",
);
const lastFetchLabel = computed(() =>
  lastFetchAt.value ? `最近获取 ${formatDateTime(lastFetchAt.value)}` : "尚未获取",
);
const domainOptions = computed(() =>
  Array.from(new Map(rows.value.map((item) => [item.domain, item.domainLabel])).entries())
    .map(([value, label]) => ({ value, label }))
    .sort((left, right) => left.label.localeCompare(right.label)),
);
const filteredRows = computed(() => {
  const query = keyword.value.trim().toLowerCase();
  return rows.value.filter((item) => {
    if (routeEventId.value && item.event_id !== routeEventId.value) return false;
    if (priorityFilter.value !== "all" && item.priority !== Number(priorityFilter.value))
      return false;
    if (domainFilter.value !== "all" && item.domain !== domainFilter.value) return false;
    if (!query) return true;
    return [
      item.event_id,
      item.topic,
      item.domainLabel,
      item.eventType,
      item.resourceId,
      item.sourceNode,
      item.operationId,
      item.correlationId,
      item.message,
    ].some((value) => value.toLowerCase().includes(query));
  });
});
const emptyText = computed(() =>
  rows.value.length ? "没有符合当前筛选条件的事件" : "当前没有事件",
);

function priorityCount(priority: number): number {
  return rows.value.filter((item) => item.priority === priority).length;
}
function priorityLabel(priority: number): string {
  return (
    ({ 0: "P0", 1: "P1", 2: "P2", 3: "P3" } as Record<number, string>)[priority] ?? `P${priority}`
  );
}
function priorityTone(priority: number): Tone {
  return priority === 0 ? "danger" : priority === 1 ? "warning" : priority === 2 ? "info" : "ready";
}
function domainLabel(value: string): string {
  return (
    (
      {
        session: "GB28181",
        stream: "流媒体",
        avai: "智能分析",
        guard: "Guard",
        integration: "三方集成",
      } as Record<string, string>
    )[value.toLowerCase()] ?? value.toUpperCase()
  );
}
function payloadObject(payload: string): Record<string, unknown> | undefined {
  try {
    const value = JSON.parse(payload);
    return value && typeof value === "object" && !Array.isArray(value)
      ? (value as Record<string, unknown>)
      : undefined;
  } catch {
    return undefined;
  }
}
function payloadText(value: unknown): string | undefined {
  return typeof value === "string" || typeof value === "number" || typeof value === "boolean"
    ? String(value)
    : undefined;
}
function payloadTimestamp(value: unknown): number | undefined {
  if (typeof value === "number" && Number.isFinite(value)) return value;
  if (typeof value === "string" && value.trim()) {
    const parsed = Number(value);
    if (Number.isFinite(parsed)) return parsed;
    const date = Date.parse(value);
    if (!Number.isNaN(date)) return date;
  }
  return undefined;
}
function decorateEvent(event: EventItem, fetchedAt: number): EventRow {
  const payload = payloadObject(event.payload);
  const topicParts = event.topic.split(".");
  const domain = (topicParts[0] || "other").toLowerCase();
  const occurredAt =
    payloadTimestamp(payload?.occurred_at_ms) ??
    payloadTimestamp(payload?.occurred_at) ??
    payloadTimestamp(payload?.timestamp);
  const resourceId =
    payloadText(payload?.resource_id) ??
    payloadText(payload?.stream_id) ??
    payloadText(payload?.device_id) ??
    payloadText(payload?.task_id) ??
    payloadText(payload?.talk_id) ??
    payloadText(payload?.playback_id) ??
    event.payload.match(/(?:resource|stream|device|task|talk|playback)_id=([^;\s]+)/)?.[1] ??
    "-";
  const sourceNode =
    payloadText(payload?.producer_node_id) ??
    payloadText(payload?.node_id) ??
    payloadText(payload?.session_node_id) ??
    payloadText(payload?.stream_node_id) ??
    "-";
  const message =
    payloadText(payload?.user_message) ??
    payloadText(payload?.message) ??
    payloadText(payload?.reason) ??
    payloadText(payload?.state) ??
    event.payload;
  let rawPayload = event.payload;
  if (payload) {
    try {
      rawPayload = JSON.stringify(payload, null, 2);
    } catch {
      /* keep original payload */
    }
  }
  return {
    ...event,
    fetchedAt,
    fetchedAtLabel: formatDateTime(fetchedAt),
    occurredAtLabel: occurredAt ? formatDateTime(occurredAt) : "-",
    priorityLabel: priorityLabel(event.priority),
    priorityTone: priorityTone(event.priority),
    domain,
    domainLabel: domainLabel(domain),
    eventType: topicParts.slice(1).join(".") || event.topic,
    resourceId,
    sourceNode,
    operationId: payloadText(payload?.operation_id) ?? "-",
    correlationId: payloadText(payload?.correlation_id) ?? "-",
    message,
    rawPayload,
  };
}
function mergeRows(target: EventRow[], incoming: EventRow[]): EventRow[] {
  const seen = new Set<string>();
  return [...incoming, ...target]
    .filter((item) => !seen.has(item.event_id) && seen.add(item.event_id))
    .slice(0, MAX_ROWS);
}
async function poll() {
  if (loading.value) return;
  loading.value = true;
  try {
    const page = await pollEvents(cursor.value || undefined, 100);
    if (page.next_after_id) cursor.value = page.next_after_id;
    const fetchedAt = Date.now();
    const known = new Set([...rows.value, ...pendingRows.value].map((item) => item.event_id));
    const incoming = page.items
      .filter((item) => !known.has(item.event_id))
      .map((item) => decorateEvent(item, fetchedAt))
      .reverse();
    if (paused.value) pendingRows.value = mergeRows(pendingRows.value, incoming);
    else rows.value = mergeRows(rows.value, incoming);
    lastFetchAt.value = fetchedAt;
  } catch (error) {
    ElMessage.error(errorMessage(error, "事件数据不可用，请稍后重试"));
  } finally {
    loading.value = false;
  }
}
function togglePaused() {
  paused.value = !paused.value;
  if (!paused.value && pendingRows.value.length) {
    rows.value = mergeRows(rows.value, pendingRows.value);
    pendingRows.value = [];
  }
}
function openDetail(row: EventRow) {
  selected.value = row;
  detailVisible.value = true;
}
async function copyEventId() {
  if (!selected.value) return;
  await navigator.clipboard.writeText(selected.value.event_id);
  ElMessage.success("Event ID 已复制");
}
async function copyPayload() {
  if (!selected.value) return;
  await navigator.clipboard.writeText(selected.value.rawPayload);
  ElMessage.success("Payload 已复制");
}
function clearRouteEvent() {
  void router.replace({ path: route.path, query: {} });
}
function schedulePolling() {
  if (timer) window.clearInterval(timer);
  timer = window.setInterval(
    () => {
      void poll();
    },
    document.hidden ? EVENT_BACKGROUND_POLLING_INTERVAL_MS : EVENT_POLLING_INTERVAL_MS,
  );
}
function handleVisibilityChange() {
  schedulePolling();
}

onMounted(() => {
  void poll();
  schedulePolling();
  document.addEventListener("visibilitychange", handleVisibilityChange);
});
onBeforeUnmount(() => {
  if (timer) window.clearInterval(timer);
  document.removeEventListener("visibilitychange", handleVisibilityChange);
});
</script>

<style scoped>
.console-state {
  display: flex;
  align-items: center;
  gap: 10px;
  color: var(--muted);
  font-size: 12px;
}
.event-summary {
  display: grid;
  grid-template-columns: repeat(4, minmax(0, 1fr));
  gap: 10px;
  margin-bottom: 16px;
}
.event-summary > div {
  padding: 13px 14px;
  border: 1px solid rgba(37, 146, 255, 0.22);
  border-radius: 13px;
  background: rgba(4, 16, 47, 0.48);
}
.event-summary span,
.event-summary small {
  display: block;
  color: var(--muted);
  font-size: 11px;
}
.event-summary b {
  display: block;
  margin: 6px 0 4px;
  color: var(--cyan);
  font-size: 22px;
}
.event-summary .is-danger b {
  color: var(--red);
}
.event-summary .is-warning b {
  color: var(--yellow);
}
.event-toolbar {
  display: grid;
  grid-template-columns: 150px 160px minmax(260px, 1fr) auto auto;
  gap: 10px;
  align-items: center;
  margin-bottom: 14px;
}
.filter-priority,
.filter-domain,
.filter-keyword {
  width: 100%;
}
.route-filter {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  margin-bottom: 12px;
  padding: 9px 12px;
  border: 1px solid rgba(5, 189, 242, 0.38);
  border-radius: 11px;
  background: rgba(5, 189, 242, 0.07);
  color: var(--button-text);
  font-size: 12px;
}
.console-foot {
  display: flex;
  justify-content: space-between;
  gap: 16px;
  margin-top: 12px;
  color: var(--faint);
  font-size: 11px;
}
:deep(.el-table__row) {
  cursor: pointer;
}
.event-detail-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 18px;
  padding-bottom: 16px;
  border-bottom: 1px solid var(--component-divider);
}
.event-detail-head small {
  color: var(--cyan);
  font-weight: 800;
  letter-spacing: 0.08em;
}
.event-detail-head h3 {
  margin: 7px 0 8px;
  font-size: 20px;
  overflow-wrap: anywhere;
}
.event-detail-head p {
  margin: 0;
  color: var(--muted);
  line-height: 1.6;
  overflow-wrap: anywhere;
}
.event-detail-kv .wide {
  grid-column: span 2;
}
.event-detail-kv b {
  overflow-wrap: anywhere;
}
.payload-section {
  margin-top: 20px;
}
.payload-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}
.payload-head h4 {
  margin: 0;
  font-size: 14px;
}
.payload-section pre {
  max-height: 320px;
  margin: 8px 0 0;
  padding: 15px;
  overflow: auto;
  border: 1px solid rgba(37, 146, 255, 0.24);
  border-radius: 13px;
  background: rgba(1, 10, 29, 0.74);
  color: #c8f3ff;
  font:
    12px/1.65 "JetBrains Mono",
    Consolas,
    monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
@media (max-width: 1000px) {
  .event-toolbar {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .filter-keyword {
    grid-column: span 2;
  }
}
@media (max-width: 700px) {
  .console-state {
    align-items: flex-end;
    flex-direction: column;
  }
  .event-summary {
    grid-template-columns: repeat(2, minmax(0, 1fr));
  }
  .event-toolbar {
    grid-template-columns: minmax(0, 1fr);
  }
  .filter-keyword {
    grid-column: span 1;
  }
  .console-foot {
    flex-direction: column;
  }
  .event-detail-kv .wide {
    grid-column: span 1;
  }
}
</style>
