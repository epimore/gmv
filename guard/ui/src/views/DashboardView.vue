<template>
  <div class="page-grid dashboard-page" v-loading="loading">
    <GlassPanel class="span-12 status-overview" :class="`is-${overallStatus.tone}`">
      <div class="status-overview-content">
        <div class="status-mark" aria-hidden="true"><span /></div>
        <div class="status-copy">
          <small>EDGE CONTROL PLANE</small>
          <h2>{{ overallStatus.title }}</h2>
          <p>{{ overallStatus.description }}</p>
        </div>
        <div class="status-meta">
          <StatusPill :label="overallStatus.label" :tone="overallStatus.tone" />
          <span>客户端刷新：{{ lastRefreshLabel }}</span>
          <el-button :loading="loading" @click="load">刷新状态</el-button>
        </div>
      </div>
    </GlassPanel>

    <MetricCard
      v-for="item in metrics"
      :key="item.label"
      :class="['span-3', `metric-${item.tone}`]"
      :label="item.label"
      :value="item.value"
      :trend="item.trend"
      :hint="item.hint"
    />

    <GlassPanel
      class="span-7"
      title="边端能力矩阵"
      subtitle="能力可发现 · 状态不混淆 · 点击进入业务页面"
    >
      <div class="capability-grid">
        <button
          v-for="item in capabilities"
          :key="item.name"
          class="capability-card"
          type="button"
          @click="router.push(item.route)"
        >
          <div class="capability-head">
            <span class="capability-index">{{ item.index }}</span>
            <StatusPill :label="item.experimental ? `实验性 · ${item.status}` : item.status" :tone="item.tone" />
          </div>
          <b>{{ item.name }}</b>
          <p>{{ item.description }}</p>
          <div class="capability-foot">
            <span>{{ item.summary }}</span>
            <i>进入 →</i>
          </div>
        </button>
      </div>
    </GlassPanel>

    <GlassPanel
      class="span-5"
      title="待处理事项"
      subtitle="仅汇总持续影响系统正常使用、需要运维介入的问题"
    >
      <div v-if="attentionItems.length" class="attention-list">
        <button
          v-for="item in attentionItems"
          :key="item.key"
          class="attention-item"
          :class="`is-${item.tone}`"
          type="button"
          @click="router.push(item.route)"
        >
          <span class="attention-dot" />
          <span class="attention-copy"
            ><b>{{ item.title }}</b
            ><small>{{ item.detail }}</small></span
          >
          <span class="attention-link">{{ item.action }} →</span>
        </button>
      </div>
      <div v-else class="attention-empty">
        <span>✓</span>
        <div><b>当前无待处理事项</b><small>已知节点、运行资源和外部投递状态正常</small></div>
      </div>
    </GlassPanel>

    <GlassPanel
      class="span-12"
      title="业务异常概览"
      subtitle="单次业务失败用于观察和下钻，不计入运维待处理事项"
    >
      <div v-if="businessExceptionItems.length" class="attention-list">
        <button
          v-for="item in businessExceptionItems"
          :key="item.key"
          class="attention-item"
          :class="`is-${item.tone}`"
          type="button"
          @click="router.push(item.route)"
        >
          <span class="attention-dot" />
          <span class="attention-copy"
            ><b>{{ item.title }}</b
            ><small>{{ item.detail }}</small></span
          >
          <span class="attention-link">{{ item.action }} →</span>
        </button>
      </div>
      <div v-else class="attention-empty">
        <span>✓</span>
        <div><b>当前无业务失败记录</b><small>点播、回放、下载等失败会在对应业务页面提供重试</small></div>
      </div>
    </GlassPanel>

    <GlassPanel v-if="experimental.enabled" class="span-12" title="最近告警" :subtitle="recentAlertsSubtitle">
      <template #action>
        <el-button @click="router.push('/events')">查看全部事件</el-button>
      </template>
      <el-table
        :data="recentAlerts"
        height="270"
        empty-text="当前无高优先级告警"
        @row-click="openEventCenter"
      >
        <el-table-column label="级别" width="100">
          <template #default="{ row }"
            ><StatusPill :label="priorityLabel(row.priority)" :tone="priorityTone(row.priority)"
          /></template>
        </el-table-column>
        <el-table-column prop="domain" label="来源" width="120" />
        <el-table-column prop="eventType" label="事件类型" min-width="220" show-overflow-tooltip />
        <el-table-column prop="resourceId" label="资源对象" min-width="180" show-overflow-tooltip />
        <el-table-column prop="message" label="摘要" min-width="280" show-overflow-tooltip />
      </el-table>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref, watch, type Ref } from "vue";
import { ElMessage } from "element-plus";
import { useRouter } from "vue-router";
import {
  listAiTasks,
  listLeases,
  listNodes,
  listOutbox,
  pollEvents,
  type AiTaskSummary,
  type EventItem,
  type LeaseInfo,
  type NodeInfo,
  type OutboxInfo,
} from "@/api/client";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";
import { useExperimentalFeaturesStore } from "@/stores/experimentalFeatures";
import { formatDateTime } from "@/utils/dateTime";

type Tone = "ready" | "warning" | "danger" | "info";
interface CapabilityItem {
  index: string;
  name: string;
  description: string;
  status: string;
  tone: Tone;
  summary: string;
  route: string;
  experimental?: boolean;
}
interface AttentionItem {
  key: string;
  title: string;
  detail: string;
  tone: Tone;
  route: string;
  action: string;
}
interface AlertRow extends EventItem {
  domain: string;
  eventType: string;
  resourceId: string;
  message: string;
}

const router = useRouter();
const experimental = useExperimentalFeaturesStore();
const loading = ref(false);
const lastRefreshAt = ref(0);
const nodes = ref<NodeInfo[]>([]);
const leases = ref<LeaseInfo[]>([]);
const aiTasks = ref<AiTaskSummary[]>([]);
const outbox = ref<OutboxInfo[]>([]);
const events = ref<EventItem[]>([]);
const nodesAvailable = ref(true);
const leasesAvailable = ref(true);
const aiAvailable = ref(true);
const outboxAvailable = ref(true);
const eventsAvailable = ref(true);

const readyNodes = computed(() => nodes.value.filter(isNodeReady));
const offlineNodes = computed(() =>
  nodes.value.filter((node) => node.connection !== "CONNECTED" || node.health === "OFFLINE"),
);
const degradedNodes = computed(() =>
  nodes.value.filter((node) => !isNodeReady(node) && !offlineNodes.value.includes(node)),
);
const runningAiTasks = computed(() => aiTasks.value.filter((item) => item.state === "running"));
const activeLeases = computed(() =>
  leases.value.filter((item) => item.state === "allocated" || item.state === "confirmed"),
);
const deadOutbox = computed(() => outbox.value.filter((item) => item.state === "dead"));
const pendingOutbox = computed(() =>
  outbox.value.filter(
    (item) => item.state === "pending" || item.state === "sending" || item.state === "retry_wait",
  ),
);
const activeLeasesOnOfflineNodes = computed(() => {
  const offlineIds = new Set(offlineNodes.value.map((node) => node.node_id));
  return leases.value.filter(
    (lease) =>
      offlineIds.has(lease.node_id) && (lease.state === "allocated" || lease.state === "confirmed"),
  );
});
const failedAiTasks = computed(() => aiTasks.value.filter((item) => item.state === "failed"));
const degradedCatalogSubscriptions = computed(() =>
  nodes.value.reduce((total, node) => {
    const value = Number(node.business_metrics.catalog_subscription_degraded_devices ?? 0);
    return total + (Number.isSafeInteger(value) && value > 0 ? value : 0);
  }, 0),
);
const dialogRuntimeConflicts = computed(() =>
  nodes.value.reduce((total, node) => {
    const value = Number(node.business_metrics.dialog_runtime_conflicts ?? 0);
    return total + (Number.isSafeInteger(value) && value > 0 ? value : 0);
  }, 0),
);

const attentionItems = computed<AttentionItem[]>(() => {
  const items: AttentionItem[] = [];
  if (!nodesAvailable.value)
    items.push({
      key: "nodes-unavailable",
      title: "节点状态不可用",
      detail: "Guard 节点目录查询失败",
      tone: "danger",
      route: "/system/health",
      action: "检查系统",
    });
  else if (!nodes.value.length)
    items.push({
      key: "nodes-empty",
      title: "尚未发现业务节点",
      detail: "等待 Session、Stream 或 Avai 节点接入",
      tone: "info",
      route: "/system/health",
      action: "查看节点",
    });
  if (!leasesAvailable.value)
    items.push({
      key: "leases-unavailable",
      title: "任务负载状态不可用",
      detail: "当前无法读取 lease 投影",
      tone: "warning",
      route: "/system/health",
      action: "检查系统",
    });
  if (offlineNodes.value.length)
    items.push({
      key: "nodes-offline",
      title: `${offlineNodes.value.length} 个节点离线`,
      detail: "连接中断或健康状态为 OFFLINE",
      tone: "danger",
      route: "/system/health",
      action: "定位节点",
    });
  if (degradedNodes.value.length)
    items.push({
      key: "nodes-degraded",
      title: `${degradedNodes.value.length} 个节点降级`,
      detail: "健康状态或调度状态异常",
      tone: "warning",
      route: "/system/health",
      action: "查看负载",
    });
  if (degradedCatalogSubscriptions.value)
    items.push({
      key: "catalog-subscriptions-degraded",
      title: `${degradedCatalogSubscriptions.value} 个设备目录订阅持续异常`,
      detail: "在线设备的 Catalog 订阅重建已连续失败约 2 分钟",
      tone: "warning",
      route: "/gb28181/register",
      action: "检查设备",
    });
  if (dialogRuntimeConflicts.value)
    items.push({
      key: "dialog-runtime-conflicts",
      title: `${dialogRuntimeConflicts.value} 个流运行态与会话记录不一致`,
      detail: "缺少安全自动清理所需的反向所有权证据，需要运维确认",
      tone: "danger",
      route: "/streams",
      action: "检查残留",
    });
  if (activeLeasesOnOfflineNodes.value.length)
    items.push({
      key: "leases-stale",
      title: `${activeLeasesOnOfflineNodes.value.length} 个任务仍关联离线节点`,
      detail: "存在非终态 lease，需要确认责任节点状态",
      tone: "danger",
      route: "/system/health",
      action: "检查任务",
    });
  if (deadOutbox.value.length)
    items.push({
      key: "outbox-dead",
      title: `${deadOutbox.value.length} 条外部投递已终止`,
      detail: "Dead Letter 需要在三方集成中处理",
      tone: "danger",
      route: "/integrations/apps",
      action: "处理投递",
    });
  if (experimental.enabled && !eventsAvailable.value)
    items.push({
      key: "events-unavailable",
      title: "告警事件数据不可用",
      detail: "当前无法确认最近高优先级事件",
      tone: "warning",
      route: "/events",
      action: "检查事件",
    });
  return items;
});

const businessExceptionItems = computed<AttentionItem[]>(() => {
  const items: AttentionItem[] = [];
  if (experimental.enabled && failedAiTasks.value.length)
    items.push({
      key: "ai-failed",
      title: `${failedAiTasks.value.length} 条失败 AI 任务记录`,
      detail: "这是业务任务结果，可进入智能分析查看或重新发起",
      tone: "warning",
      route: "/ai",
      action: "查看任务",
    });
  return items;
});

const overallStatus = computed(() => {
  if (!nodesAvailable.value)
    return {
      title: "Guard 数据暂不可用",
      description: "无法读取节点目录，请检查 Guard 服务和当前网络。",
      label: "不可用",
      tone: "danger" as Tone,
    };
  if (attentionItems.value.some((item) => item.tone === "danger"))
    return {
      title: "存在需要立即处理的问题",
      description: "关键节点、系统状态或外部投递存在异常，请从待处理事项进入定位。",
      label: "异常",
      tone: "danger" as Tone,
    };
  if (!nodes.value.length)
    return {
      title: "等待节点接入",
      description: "Guard 控制台可访问，尚未发现已注册的业务节点。",
      label: "待接入",
      tone: "info" as Tone,
    };
  if (attentionItems.value.length)
    return {
      title: "部分能力处于降级状态",
      description: "核心控制面可访问，部分系统状态需要运维关注。",
      label: "降级",
      tone: "warning" as Tone,
    };
  return {
    title: "运行正常",
    description: "已知节点、运行资源和外部投递均未发现待处理异常。",
    label: "正常",
    tone: "ready" as Tone,
  };
});

const metrics = computed(() => {
  const items = [
    {
      label: "服务节点",
      value: nodesAvailable.value ? `${readyNodes.value.length} / ${nodes.value.length}` : "—",
      trend: "READY / 全部",
      hint: offlineNodes.value.length ? `离线 ${offlineNodes.value.length}` : "连接正常",
      tone: offlineNodes.value.length ? "danger" : "ready",
    },
    {
      label: "活动租约",
      value: leasesAvailable.value ? activeLeases.value.length : "—",
      trend: leasesAvailable.value ? "ALLOCATED / CONFIRMED" : "接口不可用",
      hint: "Guard 控制面事实",
      tone: leasesAvailable.value ? "ready" : "warning",
    },
    {
      label: "待处理",
      value: attentionItems.value.length,
      trend: attentionItems.value.length ? "需要关注" : "当前正常",
      hint: attentionItems.value.some((item) => item.tone === "danger")
        ? "含关键异常"
        : "无关键异常",
      tone: attentionItems.value.some((item) => item.tone === "danger")
        ? "danger"
        : attentionItems.value.length
          ? "warning"
          : "ready",
    },
  ];
  items.push({
    label: "外部投递",
    value: outboxAvailable.value ? pendingOutbox.value.length + deadOutbox.value.length : "—",
    trend: outboxAvailable.value ? `处理中 ${pendingOutbox.value.length}` : "接口不可用",
    hint: outboxAvailable.value ? `Dead ${deadOutbox.value.length}` : "状态未知",
    tone: deadOutbox.value.length
      ? "danger"
      : pendingOutbox.value.length
        ? "warning"
        : outboxAvailable.value
          ? "ready"
          : "info",
  });
  return items;
});

const capabilities = computed<CapabilityItem[]>(() => {
  const items = [
    capabilityFromNodes("01", "GB28181", "国标设备接入、注册与监控", "/gb28181/monitor", (node) =>
      nodeMatches(node, ["session-gb28181", "gb28181"]),
    ),
    capabilityFromNodes(
      "02",
      "流媒体",
      "实时预览、回放与输出管理",
      "/streams",
      (node) => nodeMatches(node, ["stream"]),
    ),
    integrationCapability(
      "03",
      "HTTP 接入",
      "开放接口与事件回调入口",
      "/integrations/http",
      "webhook",
    ),
    integrationCapability(
      "04",
      "MQTT 接入",
      "命令订阅与事件发布入口",
      "/integrations/mqtt",
      "mqtt",
    ),
  ];
  if (!experimental.enabled) return items;
  items.push(
    {
      ...capabilityFromNodes("05", "ONVIF", "设备发现与通道控制入口", "/onvif", (node) =>
        nodeMatches(node, ["session-onvif", "onvif"]),
      ),
      experimental: true,
    },
    {
      ...capabilityFromNodes(
        "06",
        "智能分析",
        "AI 任务调度与结果摘要",
        "/ai",
        (node) => nodeMatches(node, ["avai"]),
        aiAvailable.value ? `运行 ${runningAiTasks.value.length} 个任务` : "任务数据不可用",
      ),
      experimental: true,
    },
  );
  return items;
});

const recentAlerts = computed<AlertRow[]>(() =>
  events.value
    .filter((item) => item.priority === 0 || item.priority === 1)
    .map(decorateEvent)
    .reverse()
    .slice(0, 10),
);
const lastRefreshLabel = computed(() =>
  lastRefreshAt.value ? formatDateTime(lastRefreshAt.value) : "-",
);
const recentAlertsSubtitle = computed(() =>
  eventsAvailable.value
    ? "当前事件接口返回的 P0 / P1 高优先级事件"
    : "事件数据不可用，当前无法确认最近告警",
);

function isNodeReady(node: NodeInfo): boolean {
  return (
    node.connection === "CONNECTED" && node.health === "READY" && node.scheduling === "ENABLED"
  );
}
function nodeMatches(node: NodeInfo, terms: string[]): boolean {
  const source = [node.kind, node.service, node.protocol, ...node.capabilities]
    .join(" ")
    .toLowerCase();
  return terms.some((term) => source.includes(term));
}
function capabilityFromNodes(
  index: string,
  name: string,
  description: string,
  route: string,
  matcher: (node: NodeInfo) => boolean,
  detail?: string,
): CapabilityItem {
  if (!nodesAvailable.value)
    return {
      index,
      name,
      description,
      status: "不可用",
      tone: "danger",
      summary: "节点目录不可用",
      route,
    };
  const matched = nodes.value.filter(matcher);
  if (!matched.length)
    return {
      index,
      name,
      description,
      status: "未部署",
      tone: "info",
      summary: detail ?? "未发现对应节点",
      route,
    };
  const ready = matched.filter(isNodeReady).length;
  if (ready === matched.length)
    return {
      index,
      name,
      description,
      status: "可用",
      tone: "ready",
      summary: detail ?? `${ready} 个节点就绪`,
      route,
    };
  return {
    index,
    name,
    description,
    status: ready ? "部分可用" : "降级",
    tone: "warning",
    summary: detail ?? `${ready} / ${matched.length} 个节点就绪`,
    route,
  };
}
function integrationCapability(
  index: string,
  name: string,
  description: string,
  route: string,
  kind: OutboxInfo["destination_kind"],
): CapabilityItem {
  if (!outboxAvailable.value)
    return {
      index,
      name,
      description,
      status: "不可用",
      tone: "danger",
      summary: "投递管理接口不可用",
      route,
    };
  const records = outbox.value.filter((item) => item.destination_kind === kind);
  const dead = records.filter((item) => item.state === "dead").length;
  return {
    index,
    name,
    description,
    status: "可管理",
    tone: dead ? "warning" : "info",
    summary: records.length ? `队列 ${records.length} · Dead ${dead}` : "当前无投递记录",
    route,
  };
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
function decorateEvent(event: EventItem): AlertRow {
  const payload = payloadObject(event.payload);
  const topicParts = event.topic.split(".");
  const resource =
    payloadText(payload?.resource_id) ??
    payloadText(payload?.stream_id) ??
    payloadText(payload?.device_id) ??
    payloadText(payload?.task_id) ??
    payloadText(payload?.playback_id) ??
    event.payload.match(/(?:resource|stream|device|task|playback)_id=([^;\s]+)/)?.[1] ??
    "-";
  const message =
    payloadText(payload?.user_message) ??
    payloadText(payload?.message) ??
    payloadText(payload?.reason) ??
    payloadText(payload?.state) ??
    event.payload;
  return {
    ...event,
    domain: domainLabel(topicParts[0]),
    eventType: topicParts.slice(1).join(".") || event.topic,
    resourceId: resource,
    message,
  };
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
function priorityLabel(priority: number): string {
  return (
    ({ 0: "P0 紧急", 1: "P1 告警", 2: "P2 状态", 3: "P3 遥测" } as Record<number, string>)[
      priority
    ] ?? `P${priority}`
  );
}
function priorityTone(priority: number): Tone {
  return priority === 0 ? "danger" : priority === 1 ? "warning" : "info";
}
function openEventCenter(row: AlertRow) {
  void router.push({ path: "/events", query: { event_id: row.event_id } });
}
function assignArrayResult<T>(
  result: PromiseSettledResult<T[]>,
  target: Ref<T[]>,
  available: Ref<boolean>,
) {
  available.value = result.status === "fulfilled";
  if (result.status === "fulfilled") target.value = result.value;
  else target.value = [];
}
async function load() {
  if (loading.value) return;
  loading.value = true;
  const baseResults = await Promise.allSettled([
    listNodes(),
    listLeases(),
    listOutbox(200),
  ]);
  assignArrayResult(baseResults[0], nodes, nodesAvailable);
  assignArrayResult(baseResults[1], leases, leasesAvailable);
  assignArrayResult(baseResults[2], outbox, outboxAvailable);
  let experimentalRejected = false;
  if (experimental.enabled) {
    const experimentalResults = await Promise.allSettled([
      listAiTasks(),
      pollEvents(undefined, 100),
    ]);
    assignArrayResult(experimentalResults[0], aiTasks, aiAvailable);
    if (experimentalResults[1].status === "fulfilled") {
      events.value = experimentalResults[1].value.items;
      eventsAvailable.value = true;
    } else {
      events.value = [];
      eventsAvailable.value = false;
    }
    experimentalRejected = experimentalResults.some((result) => result.status === "rejected");
  } else {
    aiTasks.value = [];
    events.value = [];
    aiAvailable.value = true;
    eventsAvailable.value = true;
  }
  lastRefreshAt.value = Date.now();
  loading.value = false;
  if (baseResults.some((result) => result.status === "rejected") || experimentalRejected)
    ElMessage.warning("部分总览数据暂不可用，页面已保留可确认的状态");
}

onMounted(() => {
  void load();
});

watch(
  () => experimental.enabled,
  () => void load(),
);
</script>

<style scoped>
.status-overview {
  border-color: rgba(53, 240, 161, 0.44);
}
.status-overview.is-warning {
  border-color: rgba(255, 209, 102, 0.64);
}
.status-overview.is-danger {
  border-color: rgba(255, 107, 138, 0.72);
}
.status-overview.is-info {
  border-color: rgba(5, 189, 242, 0.54);
}
.status-overview-content {
  display: grid;
  grid-template-columns: 66px minmax(0, 1fr) auto;
  gap: 18px;
  align-items: center;
  min-height: 100px;
}
.status-mark {
  display: grid;
  place-items: center;
  width: 58px;
  height: 58px;
  border: 1px solid currentColor;
  border-radius: 18px;
  color: var(--green);
  background: rgba(53, 240, 161, 0.08);
}
.is-warning .status-mark {
  color: var(--yellow);
  background: rgba(255, 209, 102, 0.08);
}
.is-danger .status-mark {
  color: var(--red);
  background: rgba(255, 107, 138, 0.08);
}
.is-info .status-mark {
  color: var(--cyan);
  background: rgba(5, 189, 242, 0.08);
}
.status-mark span {
  width: 17px;
  height: 17px;
  border: 4px solid currentColor;
  border-radius: 50%;
  box-shadow: 0 0 24px currentColor;
}
.status-copy small {
  color: var(--faint);
  font:
    800 11px/1 "JetBrains Mono",
    Consolas,
    monospace;
  letter-spacing: 0.18em;
}
.status-copy h2 {
  margin: 9px 0 6px;
  font-size: 24px;
}
.status-copy p {
  margin: 0;
  color: var(--muted);
  font-size: 13px;
}
.status-meta {
  display: grid;
  justify-items: end;
  gap: 9px;
  color: var(--muted);
  font-size: 12px;
}
.metric-warning :deep(.metric-value) {
  color: var(--yellow);
}
.metric-danger :deep(.metric-value) {
  color: var(--red);
}
.metric-info :deep(.metric-value) {
  color: var(--cyan);
}
.capability-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 12px;
}
.capability-card {
  min-width: 0;
  padding: 15px;
  border: 1px solid rgba(37, 146, 255, 0.3) !important;
  border-radius: 15px;
  background: rgba(4, 16, 47, 0.5) !important;
  color: var(--text) !important;
  text-align: left;
  cursor: pointer;
}
.capability-card:hover {
  border-color: rgba(103, 232, 249, 0.66) !important;
  background: rgba(8, 86, 122, 0.22) !important;
  transform: translateY(-1px);
}
.capability-head,
.capability-foot {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
}
.capability-index {
  color: var(--faint);
  font:
    800 11px/1 "JetBrains Mono",
    Consolas,
    monospace;
  letter-spacing: 0.12em;
}
.capability-card > b {
  display: block;
  margin-top: 14px;
  font-size: 16px;
}
.capability-card p {
  min-height: 36px;
  margin: 7px 0 14px;
  color: var(--muted);
  font-size: 12px;
  line-height: 1.5;
}
.capability-foot {
  padding-top: 11px;
  border-top: 1px solid rgba(37, 146, 255, 0.18);
  color: var(--muted);
  font-size: 12px;
}
.capability-foot i {
  color: var(--cyan);
  font-style: normal;
  font-weight: 800;
}
.attention-list {
  display: grid;
  gap: 9px;
  max-height: 416px;
  overflow-y: auto;
}
.attention-item {
  display: grid;
  grid-template-columns: 10px minmax(0, 1fr) auto;
  gap: 11px;
  align-items: center;
  width: 100%;
  padding: 13px;
  border: 1px solid rgba(37, 146, 255, 0.24) !important;
  border-radius: 13px;
  background: rgba(4, 16, 47, 0.5) !important;
  color: var(--text) !important;
  text-align: left;
  cursor: pointer;
}
.attention-item.is-danger {
  border-color: rgba(255, 107, 138, 0.44) !important;
}
.attention-item.is-warning {
  border-color: rgba(255, 209, 102, 0.38) !important;
}
.attention-dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  background: var(--cyan);
  box-shadow: 0 0 12px currentColor;
}
.is-danger .attention-dot {
  background: var(--red);
}
.is-warning .attention-dot {
  background: var(--yellow);
}
.attention-copy {
  display: grid;
  min-width: 0;
  gap: 4px;
}
.attention-copy b {
  font-size: 13px;
}
.attention-copy small {
  overflow: hidden;
  color: var(--muted);
  font-size: 11px;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.attention-link {
  color: var(--cyan);
  font-size: 11px;
  font-weight: 800;
}
.attention-empty {
  display: flex;
  align-items: center;
  justify-content: center;
  gap: 14px;
  min-height: 260px;
  color: var(--green);
}
.attention-empty > span {
  display: grid;
  place-items: center;
  width: 42px;
  height: 42px;
  border: 1px solid currentColor;
  border-radius: 50%;
  font-size: 20px;
}
.attention-empty div {
  display: grid;
  gap: 5px;
}
.attention-empty small {
  color: var(--muted);
}
:deep(.el-table__row) {
  cursor: pointer;
}
@media (max-width: 900px) {
  .status-overview-content {
    grid-template-columns: 48px minmax(0, 1fr);
  }
  .status-mark {
    width: 46px;
    height: 46px;
  }
  .status-meta {
    grid-column: 1 / -1;
    grid-template-columns: auto 1fr auto;
    justify-items: start;
    align-items: center;
    width: 100%;
  }
  .capability-grid {
    grid-template-columns: minmax(0, 1fr);
  }
}
@media (max-width: 560px) {
  .status-overview-content {
    grid-template-columns: minmax(0, 1fr);
  }
  .status-mark {
    display: none;
  }
  .status-meta {
    grid-template-columns: minmax(0, 1fr);
  }
  .attention-item {
    grid-template-columns: 10px minmax(0, 1fr);
  }
  .attention-link {
    grid-column: 2;
  }
}
</style>
