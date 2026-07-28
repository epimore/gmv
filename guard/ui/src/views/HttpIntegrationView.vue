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

    <GlassPanel class="span-8" title="HTTP 双向接入" subtitle="第三方调用 Guard · Guard 回调第三方">
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
          <div class="flow-meta"><span>追踪</span><code>event_id + trace_id</code></div>
        </article>
      </div>
    </GlassPanel>

    <GlassPanel class="span-4" title="HTTP 接入配置" subtitle="回调仅允许 HTTPS，私网默认拒绝">
      <template #action><el-button type="primary" :loading="saving" :disabled="!auth.isAdmin || !selectedIntegrationId" @click="saveConfig">保存配置</el-button></template>
      <div class="config-form">
        <label>接入应用</label>
        <el-select v-model="selectedIntegrationId" placeholder="请选择 HTTP 应用" @change="loadConfig">
          <el-option v-for="item in httpIntegrations" :key="item.integration_id" :label="item.name" :value="item.integration_id" />
        </el-select>
        <label>事件回调地址</label>
        <el-input v-model="callbackUrl" placeholder="https://partner.example.com/gmv/events" />
        <label>私网回调策略</label>
        <el-switch v-model="allowPrivateNetworks" active-text="仅允许白名单" inactive-text="拒绝私网" />
        <el-input
          v-if="allowPrivateNetworks"
          v-model="privateAllowlistText"
          type="textarea"
          :rows="3"
          placeholder="每行一个 hostname、IP 或 CIDR，例如 10.20.0.0/16"
        />
      </div>
      <div class="kv">
        <div class="kv-item"><span>开放 API</span><b class="code">/openapi/v1</b></div>
        <div class="kv-item"><span>协议</span><b>HTTPS</b></div>
        <div class="kv-item"><span>签名算法</span><b>HMAC-SHA256</b></div>
        <div class="kv-item"><span>幂等字段</span><b class="code">request_id</b></div>
        <div class="kv-item wide">
          <span>回调地址</span><b class="code">{{ callbackUrl || "未配置" }}</b>
        </div>
        <div class="kv-item wide"><span>凭证用途</span><b>入站验签 / 回调签名分离</b></div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-12" title="回调事件映射" subtitle="事件类型支持精确值或 * 通配；每条映射独立进入 outbox">
      <template #action>
        <div class="mapping-action">
          <el-input v-model="mappingSource" placeholder="session.* / stream.*" />
          <el-button type="primary" :disabled="!auth.isAdmin || !selectedIntegrationId || !callbackUrl" @click="addMapping">新增映射</el-button>
        </div>
      </template>
      <el-table :data="mappings" height="220" empty-text="尚未配置回调事件映射">
        <el-table-column prop="source_type" label="Guard 事件类型" min-width="220" />
        <el-table-column prop="schema_version" label="Schema" width="110" />
        <el-table-column prop="destination" label="回调地址" min-width="320" />
        <el-table-column prop="payload_profile" label="Payload Profile" min-width="160" />
        <el-table-column label="状态" width="100">
          <template #default="{ row }"><StatusPill :label="row.enabled ? '启用' : '停用'" :tone="row.enabled ? 'ready' : 'warning'" /></template>
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
          ><el-tag effect="plain">内置静态页</el-tag>
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
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useAuthStore } from "@/stores/auth";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";
import {
  errorMessage,
  getOpenApiDocument,
  getIntegrationHttpConfig,
  listIntegrations,
  listIntegrationMappings,
  saveIntegrationHttpConfig,
  saveIntegrationMapping,
  type IntegrationHttpConfig,
  type IntegrationInfo,
  type IntegrationMappingInfo,
} from "@/api/client";

const httpIntegrations = ref<IntegrationInfo[]>([]);
const selectedIntegrationId = ref("");
const auth = useAuthStore();
const callbackUrl = ref("");
const allowPrivateNetworks = ref(false);
const privateAllowlistText = ref("");
const saving = ref(false);
const config = ref<IntegrationHttpConfig | null>(null);
const mappings = ref<IntegrationMappingInfo[]>([]);
const mappingSource = ref("");
const contracts = ref<Array<{ direction: string; method: string; path: string; purpose: string; auth: string; tracking: string }>>([]);

async function loadIntegrations() {
  try {
    httpIntegrations.value = (await listIntegrations()).filter((item) => item.transport === "http");
    selectedIntegrationId.value = httpIntegrations.value[0]?.integration_id ?? "";
    await loadConfig();
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 HTTP 接入失败"));
  }
}

async function loadContract() {
  try {
    const document = await getOpenApiDocument() as { paths?: Record<string, Record<string, { summary?: string }>> };
    contracts.value = Object.entries(document.paths ?? {}).flatMap(([path, operations]) =>
      Object.entries(operations).map(([method, operation]) => ({
        direction: "被调用",
        method: method.toUpperCase(),
        path,
        purpose: operation.summary ?? "Guard 业务能力",
        auth: "Access Key + HMAC",
        tracking: method === "post" ? "request_id / operation_id" : "trace_id",
      })),
    );
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 OpenAPI 契约失败"));
  }
}

async function loadConfig() {
  if (!selectedIntegrationId.value) {
    config.value = null;
    callbackUrl.value = "";
    allowPrivateNetworks.value = false;
    privateAllowlistText.value = "";
    return;
  }
  try {
    config.value = await getIntegrationHttpConfig(selectedIntegrationId.value);
    callbackUrl.value = config.value.callback_url ?? "";
    allowPrivateNetworks.value = config.value.private_network_policy === "allowlist";
    privateAllowlistText.value = config.value.private_network_allowlist.join("\n");
    mappings.value = await listIntegrationMappings(selectedIntegrationId.value);
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 HTTP 配置失败"));
  }
}

async function addMapping() {
  const sourceType = mappingSource.value.trim();
  if (!selectedIntegrationId.value || !sourceType || !callbackUrl.value.trim()) {
    ElMessage.warning("请填写事件类型并先保存 HTTPS 回调地址");
    return;
  }
  try {
    await saveIntegrationMapping(selectedIntegrationId.value, {
      direction: "OUTBOUND",
      source_type: sourceType,
      schema_version: "v1",
      destination_kind: "HTTP",
      destination: callbackUrl.value.trim(),
      payload_profile: "event-envelope-v1",
      enabled: true,
    });
    mappings.value = await listIntegrationMappings(selectedIntegrationId.value);
    mappingSource.value = "";
    ElMessage.success("回调事件映射已新增");
  } catch (error) {
    ElMessage.error(errorMessage(error, "新增事件映射失败"));
  }
}

async function saveConfig() {
  if (!selectedIntegrationId.value || !config.value) return;
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
  window.open("/api-docs", "_blank", "noopener,noreferrer");
}

onMounted(() => {
  void loadIntegrations();
  void loadContract();
});
</script>

<style scoped>
.flow-grid {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
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

.mapping-action {
  display: grid;
  grid-template-columns: minmax(190px, 1fr) auto;
  gap: 10px;
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
