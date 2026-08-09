<template>
  <div class="page-grid">
    <MetricCard
      class="span-3"
      label="接入方向"
      value="双向"
      trend="Inbound / Callback"
      hint="按需启用"
    />
    <MetricCard class="span-3" label="认证方式" value="HMAC" trend="Access Key" hint="防重放" />
    <MetricCard class="span-3" label="接口版本" value="v1" trend="稳定契约" hint="独立于 UI API" />
    <MetricCard
      class="span-3"
      label="文档规格"
      value="OpenAPI"
      trend="随服务生成"
      hint="Guard 暴露"
    />

    <el-alert
      v-if="!selectedIntegrationId"
      class="span-12"
      title="当前未选择 HTTP 接入"
      type="info"
      :closable="false"
      show-icon
    >
      请先到 <RouterLink to="/integrations/apps">接入应用</RouterLink> 页面选择并保存 HTTP。
    </el-alert>

    <GlassPanel class="span-6" title="HTTP 双向接入" subtitle="第三方调用 Guard · Guard 回调第三方">
      <div class="flow-grid">
        <article class="flow-card inbound">
          <div class="flow-head">
            <span>INBOUND</span><StatusPill label="第三方调用" tone="info" />
          </div>
          <div class="flow-line"><b>第三方业务系统</b><i>HTTPS + HMAC</i><b>Guard Server</b></div>
          <p>
            Guard 验证 Access Key、时间戳、nonce、body hash 和签名，再将开放命令映射到内部业务操作。
          </p>
          <div class="flow-meta"><span>幂等</span><code>request_id → operation_id</code></div>
        </article>
        <article class="flow-card outbound">
          <div class="flow-head">
            <span>OUTBOUND</span><StatusPill label="事件回调" tone="ready" />
          </div>
          <div class="flow-line"><b>Guard Server</b><i>Signed POST</i><b>第三方回调地址</b></div>
          <p>事件写入有界 outbox 后异步回调；成功及时清理，失败按 TTL 重试并限量保留摘要。</p>
          <div class="flow-meta"><span>幂等</span><code>event_id</code></div>
        </article>
      </div>
    </GlassPanel>

    <GlassPanel class="span-6" title="HTTP 接入配置" subtitle="配置基础地址；实际 POST 路径会追加点转斜杠后的 event_type">
      <template #action><el-button type="primary" :loading="saving" :disabled="!auth.isAdmin || !selectedIntegrationId" @click="saveConfig">保存配置</el-button></template>
      <div class="config-form">
        <label>当前接入应用</label>
        <el-input :model-value="currentIntegration?.name ?? '未选择 HTTP'" disabled />
        <label>回调基础地址</label>
        <el-input v-model="callbackUrl" placeholder="https://partner.example.com/gmv/events 或 http://192.168.0.8/events" />
        <label>HTTP 回调策略</label>
        <el-switch v-model="allowPrivateNetworks" active-text="允许白名单地址使用 HTTP" inactive-text="HTTP 未放行" />
        <el-input
          v-if="allowPrivateNetworks"
          v-model="privateAllowlistText"
          type="textarea"
          :rows="3"
          placeholder="每行一个 hostname、IP 或 CIDR，例如 192.168.0.8、192.168.0.0/24、127.0.0.1、localhost"
        />
        <small v-if="allowPrivateNetworks" class="config-help">
          不判断公网、内网或本机：HTTP URL 的 hostname/IP 必须命中此列表。hostname 使用精确匹配；IP 可使用精确地址或 CIDR。
        </small>
      </div>
      <div class="kv">
        <div class="kv-item"><span>开放 API</span><b class="code">/openapi/v1</b></div>
        <div class="kv-item"><span>协议</span><b>HTTPS / 白名单 HTTP</b></div>
        <div class="kv-item"><span>签名算法</span><b>HMAC-SHA256</b></div>
        <div class="kv-item"><span>幂等字段</span><b class="code">request_id</b></div>
        <div class="kv-item wide">
          <span>回调地址</span><b class="code">{{ callbackUrl || "未配置" }}</b>
        </div>
        <div class="kv-item wide"><span>凭证用途</span><b>入站验签 / 回调签名分离</b></div>
      </div>
    </GlassPanel>

    <GlassPanel
      class="span-12"
      title="HMAC 凭证管理"
      subtitle="Access Key 可直接分发；Secret 默认隐藏，查看时需再次验证当前登录密码"
    >
      <template #action>
        <div class="credential-actions">
          <el-button
            :loading="creatingPurpose === 'http_inbound_verify'"
            :disabled="!auth.isAdmin || !selectedIntegrationId"
            @click="createCredential('http_inbound_verify')"
          >
            创建入站验签凭证
          </el-button>
          <el-button
            type="primary"
            :loading="creatingPurpose === 'http_callback_sign'"
            :disabled="!auth.isAdmin || !selectedIntegrationId"
            @click="createCredential('http_callback_sign')"
          >
            创建回调签名凭证
          </el-button>
        </div>
      </template>
      <el-alert
        title="入站验签和回调签名使用不同凭证；Secret 不会出现在列表接口、日志或审计详情中。"
        type="info"
        :closable="false"
        show-icon
      />
      <el-table
        v-loading="credentialsLoading"
        :data="credentials"
        class="credential-table"
        empty-text="尚未创建 HMAC 凭证"
      >
        <el-table-column label="用途" min-width="150">
          <template #default="{ row }">{{ credentialPurposeLabel(row.purpose) }}</template>
        </el-table-column>
        <el-table-column label="Access Key" min-width="280">
          <template #default="{ row }">
            <div class="secret-cell">
              <code>{{ row.access_key }}</code>
              <el-button link type="primary" @click="copyText(row.access_key, 'Access Key')">复制</el-button>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="Secret" min-width="190">
          <template #default="{ row }">
            <div class="secret-cell">
              <code>*******</code>
              <el-button
                v-if="auth.isAdmin"
                link
                type="primary"
                :disabled="row.status !== 'active'"
                @click="openRevealDialog(row)"
              >
                查看
              </el-button>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="状态" width="110">
          <template #default="{ row }">
            <StatusPill :label="row.status === 'active' ? '有效' : '已吊销'" :tone="row.status === 'active' ? 'ready' : 'warning'" />
          </template>
        </el-table-column>
        <el-table-column label="创建时间" min-width="180">
          <template #default="{ row }">{{ formatTime(row.created_at_ms) }}</template>
        </el-table-column>
        <el-table-column v-if="auth.isAdmin" label="操作" width="90" fixed="right">
          <template #default="{ row }">
            <el-button
              link
              type="danger"
              :disabled="row.status !== 'active'"
              @click="revokeCredential(row)"
            >
              吊销
            </el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-12" title="回调事件映射" subtitle="配置前置条件、事件匹配和启停状态共同决定是否进入 outbox">
      <template #action>
        <div class="mapping-action">
          <el-select
            v-model="mappingSource"
            filterable
            allow-create
            default-first-option
            placeholder="选择事件接口或输入通配规则"
          >
            <el-option
              v-for="event in callbackContracts"
              :key="event.event_type"
              :label="`${event.event_type} · ${event.summary}`"
              :value="event.event_type"
            />
          </el-select>
          <el-button type="primary" :disabled="!auth.isAdmin || !callbackReady" @click="addMapping">新增并启用</el-button>
        </div>
      </template>
      <div class="callback-readiness">
        <article v-for="item in callbackReadiness" :key="item.label" :class="{ ready: item.ready }">
          <span>{{ item.label }}</span>
          <b>{{ item.ready ? "已就绪" : "未就绪" }}</b>
          <small>{{ item.detail }}</small>
        </article>
      </div>
      <el-alert
        v-if="!callbackReady"
        class="mapping-alert"
        title="回调链路尚未就绪"
        description="请启用应用的发送回调 / 事件能力、保存符合网络策略的回调地址，并创建有效的回调签名凭证。已有映射可停用，但不会产生新的有效投递。"
        type="warning"
        :closable="false"
        show-icon
      />
      <h3 class="callback-contract-title">可回调事件接口</h3>
      <el-table :data="callbackContracts" class="callback-contract-table" empty-text="OpenAPI 暂未声明可回调事件">
        <el-table-column prop="event_type" label="事件类型" min-width="285" />
        <el-table-column label="方法" width="90"><template #default><code>POST</code></template></el-table-column>
        <el-table-column label="实际回调地址" min-width="390">
          <template #default="{ row }"><span class="code">{{ callbackContractUrl(row) }}</span></template>
        </el-table-column>
        <el-table-column label="Payload 字段" min-width="320">
          <template #default="{ row }"><span class="code">{{ callbackPayloadFields(row) }}</span></template>
        </el-table-column>
        <el-table-column prop="summary" label="用途" min-width="170" />
        <el-table-column prop="description" label="业务说明" min-width="360" />
        <el-table-column label="当前映射" width="120">
          <template #default="{ row }">
            <StatusPill
              :label="callbackEventMapped(row.event_type) ? '已映射' : '未映射'"
              :tone="callbackEventMapped(row.event_type) ? 'ready' : 'info'"
            />
          </template>
        </el-table-column>
      </el-table>
      <p class="mapping-help"><code>*</code> 只匹配一级，例如 <code>session.*</code>；<code>**</code> 匹配多级，例如 <code>integration.**</code>。通配规则也只会选择上表声明的公开事件。</p>
      <el-table :data="mappings" class="mapping-table" empty-text="尚未配置 HTTP 回调事件映射">
        <el-table-column prop="source_type" label="Guard 事件类型" min-width="220" />
        <el-table-column prop="schema_version" label="Schema" width="110" />
        <el-table-column label="实际回调地址" min-width="320">
          <template #default="{ row }"><span class="code">{{ callbackUrlForEventPath(row.source_type) }}</span></template>
        </el-table-column>
        <el-table-column prop="payload_profile" label="Payload Profile" min-width="160" />
        <el-table-column label="状态" width="180">
          <template #default="{ row }">
            <div class="mapping-state">
              <el-switch
                :model-value="row.enabled"
                :disabled="!auth.isAdmin || (!row.enabled && !callbackReady)"
                @change="setMappingEnabled(row, Boolean($event))"
              />
              <StatusPill
                :label="row.enabled ? (callbackReady ? '已启用' : '依赖未就绪') : '已停用'"
                :tone="row.enabled ? (callbackReady ? 'ready' : 'danger') : 'warning'"
              />
            </div>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-12" title="回调投递状态" subtitle="展示当前业务应用尚未清理的 webhook outbox；成功投递会及时清理，失败可在此重试">
      <template #action>
        <el-button :loading="outboxLoading" :disabled="!selectedIntegrationId" @click="loadOutbox">刷新状态</el-button>
      </template>
      <div class="delivery-summary">
        <span>处理中 <b>{{ callbackOutboxSummary.processing }}</b></span>
        <span>等待重试 <b>{{ callbackOutboxSummary.retrying }}</b></span>
        <span>Dead <b>{{ callbackOutboxSummary.dead }}</b></span>
      </div>
      <el-table v-loading="outboxLoading" :data="callbackOutbox" empty-text="当前没有待处理或失败的 HTTP 回调；成功记录会自动清理">
        <el-table-column prop="event_id" label="Event ID" min-width="230" />
        <el-table-column prop="mapping_id" label="映射" min-width="190" />
        <el-table-column label="状态" width="130">
          <template #default="{ row }">
            <StatusPill :label="outboxStateLabel(row.state)" :tone="outboxStateTone(row.state)" />
          </template>
        </el-table-column>
        <el-table-column prop="attempts" label="尝试次数" width="100" />
        <el-table-column label="最后更新" min-width="180">
          <template #default="{ row }">{{ formatTime(row.updated_at_ms) }}</template>
        </el-table-column>
        <el-table-column label="失败摘要" min-width="260">
          <template #default="{ row }">{{ row.last_error || "—" }}</template>
        </el-table-column>
        <el-table-column label="操作" width="90" fixed="right">
          <template #default="{ row }">
            <el-button
              link
              type="primary"
              :disabled="row.state !== 'dead' || !canRetryOutbox"
              @click="retryCallback(row)"
            >
              重试
            </el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel
      class="span-12"
      title="接口契约预览"
      subtitle="示例结构 · 正式内容由代码生成的 OpenAPI 提供"
    >
      <template #action><el-button @click="openDocs">查看在线文档</el-button></template>
      <el-table :data="contracts" height="270">
        <el-table-column label="方向" width="120">
          <template #default="{ row }"
            ><StatusPill
              :label="row.direction"
              :tone="row.direction === '被调用' ? 'info' : 'ready'"
          /></template>
        </el-table-column>
        <el-table-column label="方法" width="90">
          <template #default="{ row }"
            ><el-tag effect="plain">{{ row.method }}</el-tag></template
          >
        </el-table-column>
        <el-table-column label="接口 / 回调" min-width="310">
          <template #default="{ row }"
            ><span class="code">{{ row.path }}</span></template
          >
        </el-table-column>
        <el-table-column prop="purpose" label="用途" min-width="230" />
        <el-table-column prop="auth" label="鉴权" min-width="190" />
        <el-table-column prop="tracking" label="追踪字段" min-width="190" />
      </el-table>
    </GlassPanel>

    <GlassPanel
      class="span-7"
      title="文档随 Guard Server 发布"
      subtitle="代码是接口契约的唯一事实源"
    >
      <div class="document-stack">
        <div>
          <span>机器可读契约</span><code>GET /api-docs/openapi.json</code
          ><el-tag effect="plain">OpenAPI</el-tag>
        </div>
        <div>
          <span>在线接口文档</span><code>GET /api-docs/http</code
          ><el-tag effect="plain">可视化文档</el-tag>
        </div>
        <div>
          <span>能力清单</span><code>GET /api-docs/manifest.json</code
          ><el-tag effect="plain">版本 / 鉴权</el-tag>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-5" title="有限持久化" subtitle="保证稳定运行，不形成长期调用明细库">
      <div class="retention-list">
        <p><span class="keep">可靠保存</span><b>待回调任务、幂等键、短期失败摘要</b></p>
        <p><span class="clear">及时清理</span><b>成功回调、过期 nonce、终态任务</b></p>
        <p><span class="skip">不做留存</span><b>完整 header、签名、成功请求响应</b></p>
      </div>
    </GlassPanel>

    <el-dialog
      v-model="revealDialogVisible"
      title="查看 HMAC Secret"
      width="520px"
      destroy-on-close
      @closed="clearRevealDialog"
    >
      <el-alert
        title="这是敏感凭证。验证当前登录密码后，仅在本弹窗内临时展示。"
        type="warning"
        :closable="false"
        show-icon
      />
      <div class="reveal-form">
        <label>Access Key</label>
        <el-input :model-value="revealCredential?.access_key ?? ''" disabled />
        <template v-if="!revealedSecret">
          <label>当前登录密码</label>
          <el-input
            v-model="secondaryPassword"
            type="password"
            show-password
            autocomplete="current-password"
            placeholder="请输入当前登录密码"
            @keyup.enter="revealSecret"
          />
        </template>
        <template v-else>
          <label>Secret</label>
          <div class="revealed-secret">
            <code>{{ revealedSecret }}</code>
            <el-button type="primary" @click="copyText(revealedSecret, 'Secret')">复制 Secret</el-button>
          </div>
        </template>
      </div>
      <template #footer>
        <el-button @click="revealDialogVisible = false">关闭</el-button>
        <el-button
          v-if="!revealedSecret"
          type="primary"
          :loading="revealing"
          :disabled="!secondaryPassword"
          @click="revealSecret"
        >
          验证并查看
        </el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ElMessage, ElMessageBox } from "element-plus";
import { useAuthStore } from "@/stores/auth";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";
import {
  errorMessage,
  createIntegrationCredential,
  getBusinessIntegration,
  getOpenApiDocument,
  getIntegrationHttpConfig,
  listIntegrationCredentials,
  listIntegrationMappings,
  listOutbox,
  revealIntegrationCredential,
  retryOutbox,
  revokeIntegrationCredential,
  saveIntegrationHttpConfig,
  saveIntegrationMapping,
  type IntegrationCredentialInfo,
  type IntegrationHttpConfig,
  type IntegrationInfo,
  type IntegrationMappingInfo,
  type OutboxInfo,
} from "@/api/client";

interface CallbackContract {
  event_type: string;
  summary: string;
  description: string;
  method: string;
  payload_profile: string;
  http_path_suffix: string;
  payload_schema?: { properties?: Record<string, unknown> };
}

const currentIntegration = ref<IntegrationInfo | null>(null);
const selectedIntegrationId = ref("");
const auth = useAuthStore();
const callbackUrl = ref("");
const allowPrivateNetworks = ref(false);
const privateAllowlistText = ref("");
const saving = ref(false);
const config = ref<IntegrationHttpConfig | null>(null);
const mappings = ref<IntegrationMappingInfo[]>([]);
const credentials = ref<IntegrationCredentialInfo[]>([]);
const credentialsLoading = ref(false);
const outbox = ref<OutboxInfo[]>([]);
const outboxLoading = ref(false);
const creatingPurpose = ref<IntegrationCredentialInfo["purpose"] | "">("");
const revealDialogVisible = ref(false);
const revealCredential = ref<IntegrationCredentialInfo | null>(null);
const secondaryPassword = ref("");
const revealedSecret = ref("");
const revealing = ref(false);
const mappingSource = ref("");
const contracts = ref<Array<{ direction: string; method: string; path: string; purpose: string; auth: string; tracking: string }>>([]);
const callbackContracts = ref<CallbackContract[]>([]);
const savedCallbackUrl = computed(() => config.value?.callback_url?.trim() ?? "");
const callbackCredentialReady = computed(() => {
  const now = Date.now();
  return credentials.value.some(
    (credential) =>
      credential.purpose === "http_callback_sign" &&
      credential.status === "active" &&
      credential.not_before_ms <= now &&
      (credential.expires_at_ms === null || credential.expires_at_ms > now),
  );
});
const outboundApplicationReady = computed(
  () => Boolean(currentIntegration.value?.enabled && currentIntegration.value.outbound_enabled),
);
const callbackUrlReady = computed(
  () => Boolean(savedCallbackUrl.value && callbackUrl.value.trim() === savedCallbackUrl.value),
);
const callbackReady = computed(
  () => outboundApplicationReady.value && callbackUrlReady.value && callbackCredentialReady.value,
);
const callbackReadiness = computed(() => [
  {
    label: "应用出站能力",
    ready: outboundApplicationReady.value,
    detail: outboundApplicationReady.value ? "应用已启用发送回调 / 事件" : "请在接入应用页启用发送回调 / 事件",
  },
  {
    label: "回调地址",
    ready: callbackUrlReady.value,
    detail: callbackUrlReady.value ? savedCallbackUrl.value : "请保存符合网络策略的回调地址",
  },
  {
    label: "签名凭证",
    ready: callbackCredentialReady.value,
    detail: callbackCredentialReady.value ? "有效的回调签名凭证已存在" : "请创建回调签名凭证",
  },
]);
const callbackOutbox = computed(() =>
  outbox.value.filter(
    (record) =>
      record.integration_id === selectedIntegrationId.value && record.destination_kind === "webhook",
  ),
);
const callbackOutboxSummary = computed(() => ({
  processing: callbackOutbox.value.filter((record) => record.state === "pending" || record.state === "sending").length,
  retrying: callbackOutbox.value.filter((record) => record.state === "retry_wait").length,
  dead: callbackOutbox.value.filter((record) => record.state === "dead").length,
}));
const canRetryOutbox = computed(() => auth.session?.role === "operator" || auth.session?.role === "admin");

async function loadIntegration() {
  try {
    const state = await getBusinessIntegration();
    currentIntegration.value = state.integration?.transport === "http" ? state.integration : null;
    selectedIntegrationId.value = currentIntegration.value?.integration_id ?? "";
    await loadConfig();
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 HTTP 接入失败"));
  }
}

async function loadContract() {
  try {
    type ContractOperation = {
      summary?: string;
      "x-gmv-callback-url-source"?: string;
      "x-gmv-callback-path"?: string;
      "x-gmv-event-types"?: CallbackContract[];
    };
    const document = await getOpenApiDocument() as {
      paths?: Record<string, Record<string, ContractOperation>>;
      webhooks?: Record<string, Record<string, ContractOperation>>;
    };
    const inboundContracts = Object.entries(document.paths ?? {}).flatMap(([path, operations]) =>
      Object.entries(operations).map(([method, operation]) => ({
        direction: "被调用",
        method: method.toUpperCase(),
        path,
        purpose: operation.summary ?? "Guard 业务能力",
        auth: "Access Key + HMAC",
        tracking: method === "post" ? "request_id / operation_id" : "trace_id",
      })),
    );
    const callbackOperations = Object.values(document.webhooks ?? {}).flatMap((operations) =>
      Object.entries(operations).map(([method, operation]) => ({ method, operation })),
    );
    callbackContracts.value = callbackOperations.flatMap(
      ({ operation }) => operation["x-gmv-event-types"] ?? [],
    );
    contracts.value = [
      ...inboundContracts,
      ...callbackOperations.map(({ method, operation }) => ({
        direction: "回调",
        method: method.toUpperCase(),
        path: operation["x-gmv-callback-path"] ?? operation["x-gmv-callback-url-source"] ?? "HTTP 配置 callback_url",
        purpose: operation.summary ?? "Guard 事件回调",
        auth: "Callback Access Key + HMAC",
        tracking: "event_id",
      })),
    ];
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 OpenAPI 契约失败"));
  }
}

function callbackUrlForEventPath(eventType: string) {
  return appendEventPath(savedCallbackUrl.value, eventType);
}

function callbackContractUrl(event: CallbackContract) {
  return appendEventPath(savedCallbackUrl.value, event.event_type);
}

function callbackPayloadFields(event: CallbackContract) {
  return Object.keys(event.payload_schema?.properties ?? {}).join("、") || "—";
}

function appendEventPath(base: string, eventType: string) {
  const suffix = eventType.replaceAll(".", "/");
  if (!base) return `{callback_url}/${suffix}`;
  try {
    const url = new URL(base);
    url.pathname = `${url.pathname.replace(/\/+$/, "")}/${suffix}`;
    return url.toString();
  } catch {
    return `${base.replace(/\/+$/, "")}/${suffix}`;
  }
}

function callbackEventMapped(eventType: string) {
  return mappings.value.some(
    (mapping) => mapping.enabled && eventPatternMatches(mapping.source_type, eventType),
  );
}

function eventPatternMatches(pattern: string, eventType: string) {
  const patternSegments = pattern.split(".");
  const eventSegments = eventType.split(".");
  const matches = (patternIndex: number, eventIndex: number): boolean => {
    if (patternIndex === patternSegments.length) return eventIndex === eventSegments.length;
    if (patternSegments[patternIndex] === "**") {
      return matches(patternIndex + 1, eventIndex) ||
        (eventIndex < eventSegments.length && matches(patternIndex, eventIndex + 1));
    }
    return eventIndex < eventSegments.length &&
      (patternSegments[patternIndex] === "*" || patternSegments[patternIndex] === eventSegments[eventIndex]) &&
      matches(patternIndex + 1, eventIndex + 1);
  };
  return matches(0, 0);
}

async function loadConfig() {
  if (!selectedIntegrationId.value) {
    config.value = null;
    callbackUrl.value = "";
    allowPrivateNetworks.value = false;
    privateAllowlistText.value = "";
    credentials.value = [];
    mappings.value = [];
    outbox.value = [];
    return;
  }
  try {
    config.value = await getIntegrationHttpConfig(selectedIntegrationId.value);
    callbackUrl.value = config.value.callback_url ?? "";
    allowPrivateNetworks.value = config.value.private_network_policy === "allowlist";
    privateAllowlistText.value = config.value.private_network_allowlist.join("\n");
    await Promise.all([loadMappings(), loadCredentials(), loadOutbox()]);
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 HTTP 配置失败"));
  }
}

async function loadMappings() {
  if (!selectedIntegrationId.value) {
    mappings.value = [];
    return;
  }
  mappings.value = (await listIntegrationMappings(selectedIntegrationId.value)).filter(
    (mapping) => mapping.destination_kind === "HTTP",
  );
}

async function loadOutbox() {
  if (!selectedIntegrationId.value) {
    outbox.value = [];
    return;
  }
  outboxLoading.value = true;
  try {
    outbox.value = await listOutbox(500);
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载回调投递状态失败"));
  } finally {
    outboxLoading.value = false;
  }
}

async function loadCredentials() {
  if (!selectedIntegrationId.value) {
    credentials.value = [];
    return;
  }
  credentialsLoading.value = true;
  try {
    credentials.value = await listIntegrationCredentials(selectedIntegrationId.value);
  } finally {
    credentialsLoading.value = false;
  }
}

async function createCredential(purpose: IntegrationCredentialInfo["purpose"]) {
  if (!selectedIntegrationId.value || !auth.isAdmin) return;
  creatingPurpose.value = purpose;
  try {
    await createIntegrationCredential(selectedIntegrationId.value, purpose, null);
    await loadCredentials();
    ElMessage.success("HMAC 凭证已创建；Secret 默认隐藏，点击查看并完成二次鉴权");
  } catch (error) {
    ElMessage.error(errorMessage(error, "创建 HMAC 凭证失败"));
  } finally {
    creatingPurpose.value = "";
  }
}

async function revokeCredential(credential: IntegrationCredentialInfo) {
  if (!selectedIntegrationId.value || credential.status !== "active") return;
  try {
    await ElMessageBox.confirm(
      `吊销后使用 ${credential.access_key} 的三方请求将立即无法通过验证，是否继续？`,
      "吊销 HMAC 凭证",
      { type: "warning", confirmButtonText: "确认吊销", cancelButtonText: "取消" },
    );
    await revokeIntegrationCredential(selectedIntegrationId.value, credential.credential_id);
    await loadCredentials();
    ElMessage.success("HMAC 凭证已吊销");
  } catch (error) {
    if (error === "cancel" || error === "close") return;
    ElMessage.error(errorMessage(error, "吊销 HMAC 凭证失败"));
  }
}

function openRevealDialog(credential: IntegrationCredentialInfo) {
  revealCredential.value = credential;
  secondaryPassword.value = "";
  revealedSecret.value = "";
  revealDialogVisible.value = true;
}

async function revealSecret() {
  if (!selectedIntegrationId.value || !revealCredential.value || !secondaryPassword.value) return;
  revealing.value = true;
  try {
    const response = await revealIntegrationCredential(
      selectedIntegrationId.value,
      revealCredential.value.credential_id,
      secondaryPassword.value,
    );
    revealedSecret.value = response.secret;
    secondaryPassword.value = "";
  } catch (error) {
    ElMessage.error(errorMessage(error, "密码验证失败，无法查看 Secret"));
  } finally {
    revealing.value = false;
  }
}

function clearRevealDialog() {
  revealCredential.value = null;
  secondaryPassword.value = "";
  revealedSecret.value = "";
  revealing.value = false;
}

function credentialPurposeLabel(purpose: IntegrationCredentialInfo["purpose"]) {
  return purpose === "http_inbound_verify" ? "入站验签" : "回调签名";
}

function formatTime(timestampMs: number) {
  return new Date(timestampMs).toLocaleString("zh-CN", { hour12: false });
}

async function copyText(value: string, label: string) {
  try {
    await navigator.clipboard.writeText(value);
    ElMessage.success(`${label} 已复制`);
  } catch {
    ElMessage.error(`${label} 复制失败，请手动复制`);
  }
}

async function addMapping() {
  const sourceType = mappingSource.value.trim();
  if (!selectedIntegrationId.value || !sourceType || !callbackReady.value) {
    ElMessage.warning("请填写事件类型并先补齐回调链路依赖");
    return;
  }
  if (mappings.value.some((mapping) => mapping.source_type === sourceType)) {
    ElMessage.warning("相同事件类型的 HTTP 回调映射已存在，请直接启用原映射");
    return;
  }
  try {
    await saveIntegrationMapping(selectedIntegrationId.value, {
      direction: "OUTBOUND",
      source_type: sourceType,
      schema_version: "v1",
      destination_kind: "HTTP",
      destination: savedCallbackUrl.value,
      payload_profile: "event-envelope-v1",
      enabled: true,
    });
    await loadMappings();
    mappingSource.value = "";
    ElMessage.success("回调事件映射已新增");
  } catch (error) {
    ElMessage.error(errorMessage(error, "新增事件映射失败"));
  }
}

async function setMappingEnabled(mapping: IntegrationMappingInfo, enabled: boolean) {
  if (!selectedIntegrationId.value || !auth.isAdmin) return;
  if (enabled && !callbackReady.value) {
    ElMessage.warning("请先补齐回调链路依赖，再启用事件映射");
    return;
  }
  try {
    await saveIntegrationMapping(selectedIntegrationId.value, {
      mapping_id: mapping.mapping_id,
      direction: "OUTBOUND",
      source_type: mapping.source_type,
      schema_version: "v1",
      destination_kind: "HTTP",
      destination: enabled ? savedCallbackUrl.value : mapping.destination,
      payload_profile: "event-envelope-v1",
      enabled,
    });
    await loadMappings();
    ElMessage.success(enabled ? "回调事件映射已启用" : "回调事件映射已停用");
  } catch (error) {
    ElMessage.error(errorMessage(error, enabled ? "启用回调事件映射失败" : "停用回调事件映射失败"));
  }
}

async function retryCallback(record: OutboxInfo) {
  if (record.state !== "dead" || !canRetryOutbox.value) return;
  try {
    await retryOutbox(record.outbox_id);
    await loadOutbox();
    ElMessage.success("回调已重新进入投递队列");
  } catch (error) {
    ElMessage.error(errorMessage(error, "重试回调失败"));
  }
}

function outboxStateLabel(state: OutboxInfo["state"]) {
  return {
    pending: "待投递",
    sending: "投递中",
    delivered: "已投递",
    retry_wait: "等待重试",
    dead: "Dead",
  }[state];
}

function outboxStateTone(state: OutboxInfo["state"]): "info" | "ready" | "warning" | "danger" {
  if (state === "delivered") return "ready";
  if (state === "dead") return "danger";
  if (state === "retry_wait") return "warning";
  return "info";
}

function callbackConfigValidationError() {
  const value = callbackUrl.value.trim();
  if (!value) return null;
  if (value.length > 512) return "回调地址不能超过 512 个字符";
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return "请输入完整的 HTTP 或 HTTPS 回调地址";
  }
  if (url.username || url.password) return "回调地址不能包含用户名或密码";
  if (callbackContracts.value.some((event) => appendEventPath(value, event.event_type).length > 512)) {
    return "回调基础地址拼接事件路径后不能超过 512 个字符";
  }
  if (url.protocol === "https:") return null;
  if (url.protocol !== "http:") return "回调地址只支持 HTTP 或 HTTPS";
  if (!allowPrivateNetworks.value) return "使用 HTTP 时必须开启“允许白名单地址使用 HTTP”";
  if (!privateAllowlistText.value.split(/[\n,]/).some((item) => item.trim())) {
    return "使用 HTTP 时必须填写目标 hostname、IP 或 CIDR 白名单";
  }
  return null;
}

async function saveConfig() {
  if (!selectedIntegrationId.value || !config.value) return;
  if (!callbackUrl.value.trim() && mappings.value.some((mapping) => mapping.enabled)) {
    ElMessage.warning("请先停用所有回调事件映射，再清空回调地址");
    return;
  }
  const validationError = callbackConfigValidationError();
  if (validationError) {
    ElMessage.warning(validationError);
    return;
  }
  saving.value = true;
  try {
    config.value = await saveIntegrationHttpConfig(selectedIntegrationId.value, {
      callback_url: callbackUrl.value.trim() || null,
      callback_timeout_ms: config.value.callback_timeout_ms,
      private_network_policy: allowPrivateNetworks.value ? "allowlist" : "deny",
      private_network_allowlist: allowPrivateNetworks.value
        ? privateAllowlistText.value.split(/[\n,]/).map((item) => item.trim()).filter(Boolean)
        : [],
      max_attempts: config.value.max_attempts,
      event_ttl_ms: config.value.event_ttl_ms,
      max_response_bytes: config.value.max_response_bytes,
    });
    ElMessage.success("HTTP 回调配置已保存");
  } catch (error) {
    ElMessage.error(errorMessage(error, "保存 HTTP 配置失败"));
  } finally {
    saving.value = false;
  }
}

function openDocs() {
  window.open("/api-docs/http", "_blank", "noopener,noreferrer");
}

onMounted(() => {
  void loadIntegration();
  void loadContract();
});
</script>

<style scoped>
.flow-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr);
  gap: 14px;
}

.flow-card {
  min-height: 214px;
  padding: 16px;
  border: 1px solid rgba(37, 146, 255, 0.34);
  border-radius: 16px;
  background: linear-gradient(145deg, rgba(7, 31, 72, 0.78), rgba(3, 14, 41, 0.72));
}

.flow-card.inbound {
  box-shadow: inset 3px 0 var(--cyan);
}

.flow-card.outbound {
  box-shadow: inset 3px 0 var(--green);
}

.flow-head,
.flow-meta,
.flow-line {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
}

.flow-head > span {
  color: var(--faint);
  font:
    800 11px/1 "JetBrains Mono",
    Consolas,
    monospace;
  letter-spacing: 0.14em;
}

.flow-line {
  margin-top: 25px;
}

.flow-line i {
  flex: 1;
  position: relative;
  color: var(--cyan);
  font-size: 11px;
  font-style: normal;
  text-align: center;
}

.flow-line i::after {
  content: "→";
  display: block;
  margin-top: 3px;
  font-size: 20px;
}

.flow-card p {
  min-height: 46px;
  margin: 18px 0;
  color: var(--muted);
  font-size: 12px;
  line-height: 1.65;
}

.flow-meta {
  padding-top: 12px;
  border-top: 1px solid var(--component-divider);
  color: var(--muted);
  font-size: 12px;
}

.flow-meta code,
.document-stack code {
  color: #c8f3ff;
  font-family: "JetBrains Mono", Consolas, monospace;
}

.kv .wide {
  grid-column: span 2;
}

.config-form {
  display: grid;
  gap: 8px;
  margin-bottom: 14px;
}

.config-form label {
  color: var(--muted);
  font-size: 12px;
}

.config-help {
  color: var(--muted);
  font-size: 12px;
  line-height: 1.6;
}

.mapping-action {
  display: grid;
  grid-template-columns: minmax(190px, 1fr) auto;
  gap: 10px;
}

.mapping-action :deep(.el-select) {
  width: 100%;
}

.callback-readiness {
  display: grid;
  grid-template-columns: repeat(3, minmax(0, 1fr));
  gap: 10px;
}

.callback-readiness article {
  display: grid;
  gap: 5px;
  padding: 12px 14px;
  border: 1px solid rgba(255, 183, 77, 0.32);
  border-radius: 12px;
  background: rgba(71, 42, 8, 0.2);
}

.callback-readiness article.ready {
  border-color: rgba(50, 214, 164, 0.3);
  background: rgba(7, 55, 48, 0.24);
}

.callback-readiness span,
.callback-readiness small,
.mapping-help {
  color: var(--muted);
  font-size: 12px;
}

.callback-readiness b {
  color: var(--text);
}

.callback-readiness small {
  overflow-wrap: anywhere;
}

.mapping-alert,
.mapping-help,
.mapping-table {
  margin-top: 14px;
}

.callback-contract-title {
  margin: 18px 0 10px;
  color: var(--text);
  font-size: 14px;
}

.callback-contract-table {
  margin-bottom: 14px;
}

.mapping-help code {
  color: #c8f3ff;
}

.mapping-state {
  display: flex;
  align-items: center;
  gap: 10px;
}

.delivery-summary {
  display: flex;
  gap: 24px;
  margin-bottom: 14px;
  color: var(--muted);
  font-size: 12px;
}

.delivery-summary b {
  margin-left: 5px;
  color: var(--text);
}

@media (max-width: 900px) {
  .callback-readiness {
    grid-template-columns: 1fr;
  }
}

.credential-actions,
.secret-cell {
  display: flex;
  align-items: center;
  gap: 10px;
}

.credential-table {
  margin-top: 14px;
}

.secret-cell code,
.revealed-secret code {
  color: #c8f3ff;
  font-family: "JetBrains Mono", Consolas, monospace;
  overflow-wrap: anywhere;
}

.reveal-form {
  display: grid;
  gap: 10px;
  margin-top: 18px;
}

.reveal-form label {
  color: var(--muted);
  font-size: 12px;
}

.revealed-secret {
  display: grid;
  gap: 14px;
  padding: 14px;
  border: 1px solid rgba(37, 146, 255, 0.34);
  border-radius: 12px;
  background: rgba(4, 16, 47, 0.62);
}

.kv-item b {
  overflow-wrap: anywhere;
}

.document-stack {
  display: grid;
  gap: 10px;
}

.document-stack > div {
  display: grid;
  grid-template-columns: 130px minmax(220px, 1fr) auto;
  gap: 16px;
  align-items: center;
  padding: 13px 14px;
  border: 1px solid rgba(37, 146, 255, 0.24);
  border-radius: 13px;
  background: rgba(4, 16, 47, 0.48);
}

.document-stack span,
.retention-list span {
  color: var(--muted);
  font-size: 12px;
}

.retention-list {
  display: grid;
  gap: 10px;
}

.retention-list p {
  display: grid;
  grid-template-columns: 84px minmax(0, 1fr);
  gap: 12px;
  align-items: center;
  min-height: 42px;
  margin: 0;
  padding: 10px 12px;
  border-radius: 12px;
  background: rgba(4, 16, 47, 0.48);
}

.retention-list b {
  font-size: 12px;
  line-height: 1.45;
}

.retention-list .keep {
  color: var(--green);
}

.retention-list .clear {
  color: var(--cyan);
}

.retention-list .skip {
  color: var(--faint);
}
</style>
