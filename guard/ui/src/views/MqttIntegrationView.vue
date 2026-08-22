<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="Broker 连接" :value="runtime.broker_connected ? '已连接' : '未连接'" trend="真实 CONNACK" :hint="revisionHint" />
    <MetricCard class="span-3" label="协议版本" :value="protocolLabel" trend="部署级配置" hint="V3.1.1 / V5.0" />
    <MetricCard class="span-3" label="消息质量" :value="`QoS ${runtime.qos}`" trend="At least once" :hint="runtime.retain ? 'Retain 开启' : 'Retain 关闭'" />
    <MetricCard class="span-3" label="文档规格" value="AsyncAPI" trend="随服务生成" hint="Guard 暴露" />

    <GlassPanel v-if="!business?.enabled || business.transport !== 'mqtt'" class="span-12" title="当前未启用 MQTT 业务接入" subtitle="请先在“接入应用”选择 MQTT、打开“启用应用”并保存，页面随后会自动进入这里维护详细配置。">
      <el-button type="primary" @click="$router.push('/integrations/apps')">前往接入应用</el-button>
    </GlassPanel>

    <el-alert
        v-if="business?.enabled && business.transport === 'mqtt'"
        class="span-12"
        type="success"
        title="第三方业务应用已启用"
        description="Broker Runtime 保存后自动应用；连接状态由下方真实 CONNACK 独立确认，无需返回再次启用。"
        :closable="false"
        show-icon
      />
    <GlassPanel class="span-7" title="MQTT Runtime 配置" subtitle="协议版本、Broker 和凭据保存在 Guard 数据库，保存后由受管 Runtime 动态应用。">
        <el-alert :title="runtimeAlertTitle" :type="runtimeAlertType" :description="runtime.config?.last_error_summary ?? undefined" :closable="false" show-icon />
        <el-form label-position="top" class="runtime-form" :disabled="!canManageRuntime">
          <div class="form-grid">
            <el-form-item label="协议版本">
              <el-select v-model="runtimeForm.protocol_version">
                <el-option label="MQTT 3.1.1" value="v3" />
                <el-option label="MQTT 5.0" value="v5" />
              </el-select>
            </el-form-item>
            <el-form-item label="端口"><el-input-number v-model="runtimeForm.port" :min="1" :max="65535" controls-position="right" /></el-form-item>
          </div>
          <el-form-item label="Broker"><el-input v-model="runtimeForm.broker" placeholder="broker.example.com" /></el-form-item>
          <el-form-item label="Client ID"><el-input v-model="runtimeForm.client_id" placeholder="gmv-guard" /></el-form-item>
          <div class="form-grid">
            <el-form-item label="用户名"><el-input v-model="runtimeForm.username" autocomplete="off" /></el-form-item>
            <el-form-item label="密码">
              <el-input v-model="runtimePassword" type="password" show-password autocomplete="new-password" :placeholder="runtime.config?.password_configured ? '留空保持原密码' : '未配置'" />
            </el-form-item>
          </div>
          <div class="form-grid">
            <el-form-item label="TLS"><el-switch v-model="runtimeForm.tls" /></el-form-item>
            <el-form-item label="事件有效期（秒）"><el-input-number v-model="runtimeForm.publish_event_ttl_sec" :min="1" :max="2592000" controls-position="right" /></el-form-item>
          </div>
          <div class="form-actions"><el-button type="primary" :loading="savingRuntime" @click="saveRuntime">保存 Runtime 配置</el-button></div>
        </el-form>
      </GlassPanel>

      <GlassPanel class="span-5" title="Broker 连接状态" subtitle="状态由 Guard 收到真实 CONNACK 后确认，不根据配置保存结果推断。">
        <template #action><el-button :loading="refreshingRuntime" @click="refreshRuntime">刷新状态</el-button></template>
        <div class="connection-banner" :class="{ connected: runtime.broker_connected }">
          <span class="pulse" />
          <div>
            <StatusPill :label="runtime.broker_connected ? '已连接 Broker' : '未连接 Broker'" :tone="runtime.broker_connected ? 'ready' : 'warning'" />
            <small>{{ runtime.config ? `Runtime 状态：${runtime.config.apply_state}` : '尚未保存 Runtime 配置' }}</small>
          </div>
        </div>
        <div class="kv">
          <div class="kv-item wide"><span>Broker</span><b class="code">{{ brokerAddress }}</b></div>
          <div class="kv-item"><span>Desired Revision</span><b>{{ runtime.config?.desired_revision ?? '—' }}</b></div>
          <div class="kv-item"><span>Active Revision</span><b>{{ runtime.config?.active_revision ?? '—' }}</b></div>
          <div class="kv-item"><span>连接范围</span><b>单 Broker / 部署级</b></div>
          <div class="kv-item"><span>最近状态变化</span><b>{{ transitionTime }}</b></div>
        </div>
      </GlassPanel>

      <GlassPanel class="span-12" title="MQTT 双向接入" subtitle="Guard 订阅第三方命令 · Guard 发布事件和操作结果">
        <div class="flow-grid">
          <article class="flow-card subscribe">
            <div class="flow-head"><span>SUBSCRIBE</span><StatusPill label="Guard 订阅" tone="info" /></div>
            <div class="flow-line reverse"><b>第三方发布命令</b><i>Broker</i><b>Guard Server</b></div>
            <p>所有已注册 action 均可调用；Guard 仍逐条校验唯一应用、启停、方向、有效期、精确 Topic、scope、schema、TTL 和 command_id 幂等。</p>
            <div class="flow-meta"><span>命令主题</span><code>{{ topicConfig?.command_topic ?? 'gmv/commands/{integration_id}' }}</code></div>
          </article>
          <article class="flow-card publish">
            <div class="flow-head"><span>PUBLISH</span><StatusPill label="Guard 发布" tone="ready" /></div>
            <div class="flow-line"><b>Guard Server</b><i>Broker</i><b>第三方订阅</b></div>
            <p>事件和命令结果先进入有界 outbox，再以 QoS 1 发布；消费方根据 event_id 或 command_id 保证幂等。</p>
            <div class="flow-meta"><span>事件主题</span><code>{{ topicConfig?.event_topic_prefix ?? 'gmv/events/{integration_id}' }}/{event_type}</code></div>
          </article>
        </div>
      </GlassPanel>

      <GlassPanel class="span-12" title="Topic 契约预览" subtitle="以 Guard 为观察方描述 publish / subscribe 方向；完整 action 与 schema 以 AsyncAPI 为准。">
        <template #action><el-button @click="openDocs">查看在线文档</el-button></template>
        <div class="mapping-editor">
          <el-select v-model="mappingSource" filterable allow-create default-first-option placeholder="选择事件类型或输入通配规则">
            <el-option
              v-for="event in eventContracts"
              :key="event.event_type"
              :label="`${event.event_type} · ${event.summary}`"
              :value="event.event_type"
            />
          </el-select>
          <el-button type="primary" :disabled="!canManage || !topicConfig" @click="addEventMapping">新增事件映射</el-button>
        </div>
        <div v-if="eventMappings.length" class="mapping-tags">
          <el-tag v-for="mapping in eventMappings" :key="mapping.mapping_id" effect="plain">{{ mapping.source_type }} → {{ mapping.destination }}</el-tag>
        </div>
        <el-table :data="channels" height="270" empty-text="AsyncAPI 暂无 Channel">
          <el-table-column label="Guard 操作" width="130">
            <template #default="{ row }"><StatusPill :label="row.operation" :tone="row.operation === 'SUBSCRIBE' ? 'info' : 'ready'" /></template>
          </el-table-column>
          <el-table-column label="Topic" min-width="330"><template #default="{ row }"><span class="code">{{ row.topic }}</span></template></el-table-column>
          <el-table-column prop="message" label="消息模型" min-width="190" />
          <el-table-column prop="qos" label="QoS" width="90" />
          <el-table-column prop="auth" label="授权约束" min-width="190" />
          <el-table-column prop="tracking" label="幂等 / 追踪" min-width="190" />
        </el-table>
        <h3 class="event-contract-title">gmv/events/{integration_id}/{event_type} 可发布事件</h3>
        <el-table :data="eventContracts" empty-text="AsyncAPI 暂未声明可发布事件">
          <el-table-column prop="event_type" label="事件类型" min-width="285" />
          <el-table-column label="Topic" min-width="390">
            <template #default="{ row }"><span class="code">gmv/events/{integration_id}/{{ row.mqtt_topic_suffix }}</span></template>
          </el-table-column>
          <el-table-column label="Payload 字段" min-width="320">
            <template #default="{ row }"><span class="code">{{ eventPayloadFields(row) }}</span></template>
          </el-table-column>
          <el-table-column prop="summary" label="用途" min-width="180" />
          <el-table-column prop="description" label="业务说明" min-width="360" />
        </el-table>
      </GlassPanel>

      <GlassPanel class="span-7" title="文档随 Guard Server 发布" subtitle="channel、消息 schema 与运行时代码保持同步">
        <div class="document-stack">
          <div><span>机器可读契约</span><code>GET /api-docs/asyncapi.json</code><el-tag effect="plain">AsyncAPI</el-tag></div>
          <div><span>在线消息文档</span><code>GET /api-docs/mqtt</code><el-button text type="primary" @click="openDocs">打开</el-button></div>
          <div><span>能力清单</span><code>GET /api-docs/manifest.json</code><el-tag effect="plain">版本 / 鉴权</el-tag></div>
        </div>
      </GlassPanel>

      <GlassPanel class="span-5" title="边端可靠投递" subtitle="有界队列 · 重启恢复 · 资源上限">
        <div class="delivery-policy">
          <div><span>PENDING</span><p><b>等待投递</b><small>只保存必要字段和受限 payload</small></p></div>
          <div><span>RETRY</span><p><b>指数退避</b><small>按 TTL 和最大次数控制重试</small></p></div>
          <div><span>DEAD</span><p><b>短期限量</b><small>保留失败摘要，支持人工重试</small></p></div>
          <div><span>DONE</span><p><b>及时清理</b><small>成功终态不形成长期调用历史</small></p></div>
        </div>
      </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import StatusPill from '@/components/StatusPill.vue';
import {
  errorMessage,
  getAsyncApiDocument,
  getBusinessIntegration,
  getIntegrationMqttConfig,
  getIntegrationMqttRuntime,
  listIntegrationMappings,
  saveIntegrationMapping,
  saveIntegrationMqttRuntime,
  type IntegrationInfo,
  type IntegrationMappingInfo,
  type IntegrationMqttConfig,
  type IntegrationMqttRuntime,
} from '@/api/client';
import { useAuthStore } from '@/stores/auth';

type TopicChannel = { operation: string; topic: string; message: string; qos: string; auth: string; tracking: string };
type EventContract = {
  event_type: string;
  mqtt_topic_suffix: string;
  summary: string;
  description: string;
  payload_profile: string;
  payload_schema?: { properties?: Record<string, unknown> };
};

const auth = useAuthStore();
const business = ref<IntegrationInfo | null>(null);
const topicConfig = ref<IntegrationMqttConfig | null>(null);
const eventMappings = ref<IntegrationMappingInfo[]>([]);
const channels = ref<TopicChannel[]>([]);
const eventContracts = ref<EventContract[]>([]);

function eventPayloadFields(event: EventContract) {
  return Object.keys(event.payload_schema?.properties ?? {}).join('、') || '—';
}

const mappingSource = ref('');
const runtime = reactive<IntegrationMqttRuntime>({ configured: false, broker_connected: false, config: null, connection_scope: 'deployment', qos: 1, retain: false });
const runtimeForm = reactive({ protocol_version: 'v5' as 'v3' | 'v5', broker: '', port: 8883, client_id: '', username: '', tls: true, publish_event_ttl_sec: 86400 });
const runtimePassword = ref('');
const savingRuntime = ref(false);
const refreshingRuntime = ref(false);
const canManage = computed(() => auth.session?.role === 'admin');
const canManageRuntime = computed(() => Boolean(
  canManage.value && business.value?.enabled && business.value.transport === 'mqtt',
));
const protocolLabel = computed(() => runtime.config?.protocol_version === 'v3' ? 'V3.1.1' : runtime.config?.protocol_version === 'v5' ? 'V5.0' : '—');
const revisionHint = computed(() => runtime.config ? `Desired ${runtime.config.desired_revision} / Active ${runtime.config.active_revision ?? '—'}` : 'Desired / Active');
const runtimeAlertTitle = computed(() => !runtime.config ? 'MQTT Runtime 尚未配置' : runtime.broker_connected ? '当前配置已成功连接 Broker' : `尚未连接 Broker · ${runtime.config.apply_state}`);
const runtimeAlertType = computed(() => runtime.broker_connected ? 'success' : runtime.config?.apply_state === 'DEGRADED' ? 'error' : 'warning');
const brokerAddress = computed(() => runtime.config ? `${runtime.config.broker}:${runtime.config.port}` : '—');
const transitionTime = computed(() => runtime.config?.last_transition_at_ms ? new Date(runtime.config.last_transition_at_ms).toLocaleString() : '—');

function requestId(): string { return crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`; }

function applyRuntime(value: IntegrationMqttRuntime): void {
  Object.assign(runtime, value);
  if (!value.config) return;
  Object.assign(runtimeForm, {
    protocol_version: value.config.protocol_version,
    broker: value.config.broker,
    port: value.config.port,
    client_id: value.config.client_id,
    username: value.config.username ?? '',
    tls: value.config.tls,
    publish_event_ttl_sec: value.config.publish_event_ttl_sec,
  });
}

function applyContract(document: Record<string, unknown>): void {
  const contract = document as {
    channels?: Record<string, {
      address?: string;
      messages?: Record<string, unknown>;
      'x-gmv-event-types'?: EventContract[];
    }>;
  };
  channels.value = Object.entries(contract.channels ?? {}).map(([name, channel]) => ({
    operation: name === 'commands' ? 'SUBSCRIBE' : 'PUBLISH',
    topic: channel.address ?? name,
    message: Object.keys(channel.messages ?? {})[0] ?? 'Message',
    qos: '1',
    auth: name === 'commands' ? '唯一应用 + Scope' : 'Topic ACL + Mapping',
    tracking: name === 'commands' || name === 'commandResults' ? 'command_id / operation_id' : 'event_id',
  }));
  eventContracts.value = contract.channels?.events?.['x-gmv-event-types'] ?? [];
}

async function load(): Promise<void> {
  try {
    const [appState, runtimeValue, contract] = await Promise.all([
      getBusinessIntegration(),
      getIntegrationMqttRuntime(),
      getAsyncApiDocument(),
    ]);
    business.value = appState.integration;
    applyRuntime(runtimeValue);
    applyContract(contract);
    runtimePassword.value = '';
    if (!business.value || business.value.transport !== 'mqtt') return;
    const [topicValue, mappings] = await Promise.all([
      getIntegrationMqttConfig(business.value.integration_id),
      listIntegrationMappings(business.value.integration_id),
    ]);
    topicConfig.value = topicValue;
    eventMappings.value = mappings.filter((mapping) => mapping.destination_kind === 'MQTT' && mapping.direction === 'OUTBOUND');
  } catch (error) {
    ElMessage.error(errorMessage(error, '加载 MQTT 接入配置失败'));
  }
}

async function refreshRuntime(): Promise<void> {
  refreshingRuntime.value = true;
  try {
    applyRuntime(await getIntegrationMqttRuntime());
  } catch (error) {
    ElMessage.error(errorMessage(error, '刷新 Broker 连接状态失败'));
  } finally {
    refreshingRuntime.value = false;
  }
}

async function saveRuntime(): Promise<void> {
  if (!canManageRuntime.value) return;
  if (!runtimeForm.broker.trim() || !runtimeForm.client_id.trim()) {
    ElMessage.warning('请填写 Broker 和 Client ID');
    return;
  }
  if (runtimeForm.username.trim() && !runtimePassword.value && !runtime.config?.password_configured) {
    ElMessage.warning('首次配置账号认证时必须填写密码');
    return;
  }
  savingRuntime.value = true;
  try {
    runtime.config = await saveIntegrationMqttRuntime({
      request_id: requestId(),
      protocol_version: runtimeForm.protocol_version,
      broker: runtimeForm.broker.trim(),
      port: runtimeForm.port,
      client_id: runtimeForm.client_id.trim(),
      username: runtimeForm.username.trim() || null,
      ...(runtimePassword.value ? { password: runtimePassword.value } : {}),
      tls: runtimeForm.tls,
      publish_event_ttl_sec: runtimeForm.publish_event_ttl_sec,
      expected_config_version: runtime.config?.config_version ?? 0,
    });
    runtime.configured = true;
    runtime.broker_connected = false;
    runtimePassword.value = '';
    ElMessage.success('MQTT Runtime 配置已保存，等待 Broker 连接确认');
    window.setTimeout(() => void refreshRuntime(), 1200);
  } catch (error) {
    ElMessage.error(errorMessage(error, '保存 MQTT Runtime 配置失败'));
  } finally {
    savingRuntime.value = false;
  }
}

async function addEventMapping(): Promise<void> {
  const sourceType = mappingSource.value.trim();
  if (!business.value || !topicConfig.value || !sourceType) {
    ElMessage.warning('请填写外发事件类型');
    return;
  }
  try {
    await saveIntegrationMapping(business.value.integration_id, {
      direction: 'OUTBOUND',
      source_type: sourceType,
      schema_version: 'v1',
      destination_kind: 'MQTT',
      destination: `${topicConfig.value.event_topic_prefix}/${sourceType.replaceAll('.', '/')}`,
      payload_profile: 'event-envelope-v1',
      enabled: true,
    });
    mappingSource.value = '';
    eventMappings.value = (await listIntegrationMappings(business.value.integration_id)).filter(
      (mapping) => mapping.destination_kind === 'MQTT' && mapping.direction === 'OUTBOUND',
    );
    ElMessage.success('MQTT 事件映射已新增');
  } catch (error) {
    ElMessage.error(errorMessage(error, '新增 MQTT 事件映射失败'));
  }
}

function openDocs(): void {
  window.open('/api-docs/mqtt', '_blank', 'noopener,noreferrer');
}

onMounted(load);
</script>

<style scoped>
.runtime-form { margin-top: 18px; }
.form-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 16px; }
.form-actions { display: flex; justify-content: flex-end; }
.connection-banner { display: flex; gap: 12px; align-items: center; margin-bottom: 14px; padding: 14px; border: 1px solid rgba(255, 190, 68, 0.34); border-radius: 14px; background: rgba(116, 73, 5, 0.16); }
.connection-banner.connected { border-color: rgba(53, 240, 161, 0.32); background: rgba(8, 92, 70, 0.16); }
.connection-banner .pulse { width: 10px; height: 10px; border-radius: 50%; background: var(--yellow); box-shadow: 0 0 14px var(--yellow); }
.connection-banner.connected .pulse { background: var(--green); box-shadow: 0 0 18px var(--green); }
.connection-banner > div { display: grid; gap: 6px; }
.connection-banner small { color: var(--muted); }
.kv .wide { grid-column: span 2; }
.kv-item b { overflow-wrap: anywhere; }
.flow-grid { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 14px; }
.flow-card { min-height: 214px; padding: 16px; border: 1px solid rgba(37, 146, 255, 0.34); border-radius: 16px; background: linear-gradient(145deg, rgba(7, 31, 72, 0.78), rgba(3, 14, 41, 0.72)); }
.flow-card.subscribe { box-shadow: inset 3px 0 var(--cyan); }
.flow-card.publish { box-shadow: inset 3px 0 var(--green); }
.flow-head, .flow-meta, .flow-line { display: flex; align-items: center; justify-content: space-between; gap: 12px; }
.flow-head > span { color: var(--faint); font: 800 11px/1 "JetBrains Mono", Consolas, monospace; letter-spacing: 0.14em; }
.flow-line { margin-top: 25px; }
.flow-line i { flex: 1; color: var(--cyan); font-size: 11px; font-style: normal; text-align: center; }
.flow-line i::after { content: "→"; display: block; margin-top: 3px; font-size: 20px; }
.flow-card p { min-height: 46px; margin: 18px 0; color: var(--muted); font-size: 12px; line-height: 1.65; }
.flow-meta { padding-top: 12px; border-top: 1px solid var(--component-divider); color: var(--muted); font-size: 12px; }
.flow-meta code, .document-stack code { color: #c8f3ff; font-family: "JetBrains Mono", Consolas, monospace; }
.mapping-editor { display: grid; grid-template-columns: minmax(240px, 1fr) auto; gap: 10px; margin-bottom: 10px; }
.mapping-editor :deep(.el-select) { width: 100%; }
.mapping-tags { display: flex; flex-wrap: wrap; gap: 6px 14px; margin-bottom: 12px; }
.event-contract-title { margin: 20px 0 10px; color: var(--text); font-size: 14px; }
.document-stack { display: grid; gap: 10px; }
.document-stack > div { display: grid; grid-template-columns: 130px minmax(220px, 1fr) auto; gap: 16px; align-items: center; padding: 13px 14px; border: 1px solid rgba(37, 146, 255, 0.24); border-radius: 13px; background: rgba(4, 16, 47, 0.48); }
.document-stack span { color: var(--muted); font-size: 12px; }
.delivery-policy { display: grid; grid-template-columns: repeat(2, minmax(0, 1fr)); gap: 10px; }
.delivery-policy > div { display: grid; grid-template-columns: 54px minmax(0, 1fr); gap: 10px; align-items: center; min-height: 72px; padding: 11px; border: 1px solid rgba(37, 146, 255, 0.22); border-radius: 12px; background: rgba(4, 16, 47, 0.48); }
.delivery-policy > div > span { color: var(--cyan); font: 800 10px/1 "JetBrains Mono", Consolas, monospace; }
.delivery-policy p { display: grid; gap: 4px; margin: 0; }
.delivery-policy small { color: var(--muted); line-height: 1.35; }
@media (max-width: 760px) {
  .form-grid, .flow-grid { grid-template-columns: 1fr; gap: 0; }
  .mapping-editor, .document-stack > div { grid-template-columns: 1fr; }
}
</style>
