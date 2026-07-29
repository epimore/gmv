<template>
  <div class="page-grid">
    <MetricCard
      class="span-3"
      label="协议版本"
      :value="protocolLabel"
      trend="MQTT V3 / V5"
      hint="应用级显式选择"
    />
    <MetricCard class="span-3" label="传输安全" value="TLS" trend="Broker 鉴权" hint="Topic ACL" />
    <MetricCard
      class="span-3"
      label="消息质量"
      value="QoS 1"
      trend="At least once"
      hint="业务幂等"
    />
    <MetricCard
      class="span-3"
      label="文档规格"
      value="AsyncAPI"
      trend="随服务生成"
      hint="Guard 暴露"
    />

    <GlassPanel
      class="span-8"
      title="MQTT 双向接入"
      subtitle="Guard 订阅第三方命令 · Guard 发布事件和操作结果"
    >
      <div class="flow-grid">
        <article class="flow-card subscribe">
          <div class="flow-head">
            <span>SUBSCRIBE</span><StatusPill label="Guard 订阅" tone="info" />
          </div>
          <div class="flow-line reverse"><b>第三方发布命令</b><i>Broker</i><b>Guard Server</b></div>
          <p>
            按允许的 topic 接收命令，完成来源、权限、schema、TTL 和 command_id
            幂等校验后进入内部业务流程。
          </p>
          <div class="flow-meta">
            <span>命令主题</span><code>gmv/commands/{integration_id}</code>
          </div>
        </article>
        <article class="flow-card publish">
          <div class="flow-head">
            <span>PUBLISH</span><StatusPill label="Guard 发布" tone="ready" />
          </div>
          <div class="flow-line"><b>Guard Server</b><i>Broker</i><b>第三方订阅</b></div>
          <p>
            事件和命令结果先进入有界 outbox，再以 QoS 1 发布；消费方根据 event_id 或 command_id
            保证幂等。
          </p>
          <div class="flow-meta"><span>事件主题</span><code>gmv/events/{integration_id}/{event_type}</code></div>
        </article>
      </div>
    </GlassPanel>

    <GlassPanel class="span-4" title="Broker 连接配置" subtitle="协议版本必须显式选择，不自动推断">
      <template #action>
        <el-button type="primary" :loading="saving" :disabled="!auth.isAdmin || !selectedIntegrationId" @click="saveProtocol">保存配置</el-button>
      </template>
      <div class="integration-selector">
        <span>接入应用</span>
        <el-select v-model="selectedIntegrationId" placeholder="请选择 MQTT 应用" @change="loadMqttConfig">
          <el-option
            v-for="item in mqttIntegrations"
            :key="item.integration_id"
            :label="item.name"
            :value="item.integration_id"
          />
        </el-select>
      </div>
      <div class="protocol-selector">
        <span>MQTT 协议版本</span>
        <el-radio-group v-model="protocolVersion" aria-label="MQTT 协议版本">
          <el-radio-button value="v3">V3.1.1</el-radio-button>
          <el-radio-button value="v5">V5.0</el-radio-button>
        </el-radio-group>
        <small>用于明确三方契约版本；必须与 Guard 当前部署的 Broker runtime 版本一致。</small>
      </div>
      <div class="protocol-selector">
        <span>允许的入站命令</span>
        <el-checkbox-group v-model="allowedActions" class="action-selector">
          <el-checkbox v-for="action in actionOptions" :key="action" :value="action">{{ action }}</el-checkbox>
        </el-checkbox-group>
        <small>命令 topic、integration_id 和 action 三者同时校验，避免应用间串用。</small>
      </div>
      <div class="broker-status">
        <span class="pulse" />
        <div>
          <b>{{ runtimeStatus }}</b>
          <small>{{ selectedIntegrationId || "请先在应用与凭证页创建 MQTT 接入" }}</small>
        </div>
      </div>
      <div class="kv">
        <div class="kv-item wide">
          <span>Broker</span><b>部署级连接（地址不在应用配置中暴露）</b>
        </div>
        <div class="kv-item"><span>应用 ID</span><b class="code">{{ selectedIntegrationId || "—" }}</b></div>
        <div class="kv-item"><span>协议版本</span><b>{{ protocolLabel }}</b></div>
        <div class="kv-item"><span>连接范围</span><b>单 Broker / 部署级</b></div>
        <div class="kv-item"><span>投递语义</span><b>QoS {{ mqttRuntime?.qos ?? 1 }} / Retain 关闭</b></div>
        <div class="kv-item"><span>消息安全</span><b>Broker TLS + Topic ACL</b></div>
      </div>
    </GlassPanel>

    <GlassPanel
      class="span-12"
      title="Topic 契约预览"
      subtitle="以 Guard 为观察方描述 publish / subscribe 方向"
    >
      <template #action><el-button @click="openDocs">查看在线文档</el-button></template>
      <div class="mapping-editor">
        <el-input v-model="mappingSource" placeholder="外发事件类型，例如 stream.started" />
        <el-button type="primary" :disabled="!auth.isAdmin || !selectedIntegrationId" @click="addEventMapping">新增事件映射</el-button>
      </div>
      <div v-if="eventMappings.length" class="mapping-tags">
        <el-tag v-for="mapping in eventMappings" :key="mapping.mapping_id" effect="plain">
          {{ mapping.source_type }} → {{ mapping.destination }}
        </el-tag>
      </div>
      <el-table :data="channels" height="270">
        <el-table-column label="Guard 操作" width="130">
          <template #default="{ row }"
            ><StatusPill
              :label="row.operation"
              :tone="row.operation === 'SUBSCRIBE' ? 'info' : 'ready'"
          /></template>
        </el-table-column>
        <el-table-column label="Topic" min-width="330">
          <template #default="{ row }"
            ><span class="code">{{ row.topic }}</span></template
          >
        </el-table-column>
        <el-table-column prop="message" label="消息模型" min-width="190" />
        <el-table-column prop="qos" label="QoS" width="90" />
        <el-table-column prop="auth" label="授权约束" min-width="180" />
        <el-table-column prop="tracking" label="幂等 / 追踪" min-width="190" />
      </el-table>
    </GlassPanel>

    <GlassPanel
      class="span-7"
      title="文档随 Guard Server 发布"
      subtitle="channel、消息 schema 与运行时代码保持同步"
    >
      <div class="document-stack">
        <div>
          <span>机器可读契约</span><code>GET /api-docs/asyncapi.json</code
          ><el-tag effect="plain">AsyncAPI</el-tag>
        </div>
        <div>
          <span>在线消息文档</span><code>GET /api-docs/mqtt</code
          ><el-tag effect="plain">可视化文档</el-tag>
        </div>
        <div>
          <span>能力清单</span><code>GET /api-docs/manifest.json</code
          ><el-tag effect="plain">版本 / 鉴权</el-tag>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-5" title="边端可靠投递" subtitle="有界队列 · 重启恢复 · 资源上限">
      <div class="delivery-policy">
        <div>
          <span>PENDING</span>
          <p><b>等待投递</b><small>只保存必要字段和受限 payload</small></p>
        </div>
        <div>
          <span>RETRY</span>
          <p><b>指数退避</b><small>按 TTL 和最大次数控制重试</small></p>
        </div>
        <div>
          <span>DEAD</span>
          <p><b>短期限量</b><small>保留失败摘要，支持人工重试</small></p>
        </div>
        <div>
          <span>DONE</span>
          <p><b>及时清理</b><small>成功终态不形成长期调用历史</small></p>
        </div>
      </div>
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, ref } from "vue";
import { ElMessage } from "element-plus";
import { useAuthStore } from "@/stores/auth";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";
import {
  errorMessage,
  getAsyncApiDocument,
  getIntegrationMqttConfig,
  getIntegrationMqttRuntime,
  listIntegrationMappings,
  listIntegrations,
  saveIntegrationMapping,
  saveIntegrationMqttConfig,
  type IntegrationInfo,
  type IntegrationMqttConfig,
  type IntegrationMqttRuntime,
  type IntegrationMappingInfo,
} from "@/api/client";

const protocolVersion = ref<"v3" | "v5">("v3");
const auth = useAuthStore();
const protocolLabel = computed(() => (protocolVersion.value === "v5" ? "V5.0" : "V3.1.1"));
const mqttIntegrations = ref<IntegrationInfo[]>([]);
const selectedIntegrationId = ref("");
const saving = ref(false);
const mqttConfig = ref<IntegrationMqttConfig | null>(null);
const allowedActions = ref<string[]>([]);
const actionOptions = ["stream.start", "stream.stop", "stream.playback", "stream.download", "device.talk", "device.ptz", "ai.start", "ai.cancel", "playback.ticket.renew"];
const mappingSource = ref("");
const eventMappings = ref<IntegrationMappingInfo[]>([]);
const mqttRuntime = ref<IntegrationMqttRuntime | null>(null);
const runtimeStatus = computed(() => {
  if (!mqttRuntime.value?.enabled) return "部署级 MQTT runtime 未启用";
  const version = mqttRuntime.value.protocol_version === "v5" ? "V5.0" : "V3.1.1";
  return `部署级 MQTT runtime 已启用 · ${version}`;
});
const channels = ref<
  Array<{
    operation: string;
    topic: string;
    message: string;
    qos: string;
    auth: string;
    tracking: string;
  }>
>([]);

async function loadIntegrations() {
  try {
    const [integrations, runtime] = await Promise.all([listIntegrations(), getIntegrationMqttRuntime()]);
    mqttRuntime.value = runtime;
    protocolVersion.value = runtime.protocol_version;
    mqttIntegrations.value = integrations.filter((item) => item.transport === "mqtt");
    selectedIntegrationId.value = mqttIntegrations.value[0]?.integration_id ?? "";
    await loadMqttConfig();
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 MQTT 接入失败"));
  }
}

async function loadMqttConfig() {
  if (!selectedIntegrationId.value) {
    mqttConfig.value = null;
    allowedActions.value = [];
    eventMappings.value = [];
    protocolVersion.value = mqttRuntime.value?.protocol_version ?? "v3";
    return;
  }
  try {
    mqttConfig.value = await getIntegrationMqttConfig(selectedIntegrationId.value);
    protocolVersion.value = mqttConfig.value.protocol_version;
    allowedActions.value = [...mqttConfig.value.allowed_actions];
    eventMappings.value = (await listIntegrationMappings(selectedIntegrationId.value)).filter(
      (mapping) => mapping.destination_kind === "MQTT" && mapping.direction === "OUTBOUND",
    );
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 MQTT 配置失败"));
  }
}

async function loadContract() {
  try {
    const document = (await getAsyncApiDocument()) as {
      channels?: Record<string, { address?: string; messages?: Record<string, unknown> }>;
    };
    channels.value = Object.entries(document.channels ?? {}).map(([name, channel]) => ({
      operation: name === "commands" ? "SUBSCRIBE" : "PUBLISH",
      topic: channel.address ?? name,
      message: Object.keys(channel.messages ?? {})[0] ?? "Message",
      qos: "1",
      auth: "Broker ACL",
      tracking:
        name === "commands" || name === "commandResults"
          ? "command_id / operation_id"
          : "event_id / trace_id",
    }));
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载 AsyncAPI 契约失败"));
  }
}

async function saveProtocol() {
  if (!selectedIntegrationId.value || !mqttConfig.value) return;
  saving.value = true;
  try {
    mqttConfig.value = await saveIntegrationMqttConfig(selectedIntegrationId.value, {
      protocol_version: protocolVersion.value,
      allowed_actions: allowedActions.value,
      command_topic: mqttConfig.value.command_topic,
      result_topic: mqttConfig.value.result_topic,
      event_topic_prefix: mqttConfig.value.event_topic_prefix,
    });
    ElMessage.success(`MQTT 协议版本已保存为 ${protocolLabel.value}`);
  } catch (error) {
    ElMessage.error(errorMessage(error, "保存 MQTT 协议版本失败"));
  } finally {
    saving.value = false;
  }
}

async function addEventMapping() {
  const sourceType = mappingSource.value.trim();
  if (!selectedIntegrationId.value || !mqttConfig.value || !sourceType) {
    ElMessage.warning("请填写外发事件类型");
    return;
  }
  try {
    await saveIntegrationMapping(selectedIntegrationId.value, {
      direction: "OUTBOUND",
      source_type: sourceType,
      schema_version: "v1",
      destination_kind: "MQTT",
      destination: `${mqttConfig.value.event_topic_prefix}/${sourceType.replaceAll(".", "/")}`,
      payload_profile: "event-envelope-v1",
      enabled: true,
    });
    mappingSource.value = "";
    eventMappings.value = (await listIntegrationMappings(selectedIntegrationId.value)).filter(
      (mapping) => mapping.destination_kind === "MQTT" && mapping.direction === "OUTBOUND",
    );
    ElMessage.success("MQTT 事件映射已新增");
  } catch (error) {
    ElMessage.error(errorMessage(error, "新增 MQTT 事件映射失败"));
  }
}

onMounted(() => {
  void loadIntegrations();
  void loadContract();
});

function openDocs() {
  window.open("/api-docs/mqtt", "_blank", "noopener,noreferrer");
}
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

.flow-card.subscribe {
  box-shadow: inset 3px 0 var(--cyan);
}

.flow-card.publish {
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

.broker-status {
  display: flex;
  gap: 12px;
  align-items: center;
  margin-bottom: 14px;
  padding: 13px 14px;
  border: 1px solid rgba(53, 240, 161, 0.32);
  border-radius: 14px;
  background: rgba(8, 92, 70, 0.16);
}

.protocol-selector {
  display: grid;
  gap: 10px;
  margin-bottom: 14px;
  padding: 13px 14px;
  border: 1px solid rgba(37, 146, 255, 0.3);
  border-radius: 14px;
  background: rgba(4, 16, 47, 0.48);
}

.integration-selector {
  display: grid;
  gap: 8px;
  margin-bottom: 12px;
}

.integration-selector > span {
  color: var(--muted);
  font-size: 12px;
}

.protocol-selector > span {
  color: var(--muted);
  font-size: 12px;
}

.protocol-selector small {
  color: var(--faint);
  line-height: 1.45;
}

.action-selector,
.mapping-tags {
  display: flex;
  flex-wrap: wrap;
  gap: 6px 14px;
}

.mapping-editor {
  display: grid;
  grid-template-columns: minmax(240px, 1fr) auto;
  gap: 10px;
  margin-bottom: 10px;
}

.mapping-tags {
  margin-bottom: 12px;
}

.broker-status .pulse {
  width: 10px;
  height: 10px;
  border-radius: 50%;
  background: var(--green);
  box-shadow: 0 0 18px var(--green);
}

.broker-status div {
  display: grid;
  gap: 4px;
}

.broker-status small {
  color: var(--muted);
}

.kv .wide {
  grid-column: span 2;
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

.document-stack span {
  color: var(--muted);
  font-size: 12px;
}

.delivery-policy {
  display: grid;
  grid-template-columns: repeat(2, minmax(0, 1fr));
  gap: 10px;
}

.delivery-policy > div {
  display: grid;
  grid-template-columns: 54px minmax(0, 1fr);
  gap: 10px;
  align-items: center;
  min-height: 72px;
  padding: 11px;
  border: 1px solid rgba(37, 146, 255, 0.22);
  border-radius: 12px;
  background: rgba(4, 16, 47, 0.48);
}

.delivery-policy > div > span {
  color: var(--cyan);
  font:
    800 10px/1 "JetBrains Mono",
    Consolas,
    monospace;
}

.delivery-policy p {
  display: grid;
  gap: 4px;
  margin: 0;
}

.delivery-policy small {
  color: var(--muted);
  line-height: 1.35;
}

@media (max-width: 720px) {
  .mapping-editor {
    grid-template-columns: 1fr;
  }
}
</style>
