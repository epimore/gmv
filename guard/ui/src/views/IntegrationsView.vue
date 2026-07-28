<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="接入应用" :value="applications.length" trend="HTTP / MQTT" hint="方式单选" />
    <MetricCard class="span-3" label="有效凭证" :value="activeCredentialCount" trend="分方向授权" hint="HMAC / Broker" />
    <MetricCard class="span-3" label="MQTT 应用" :value="mqttCount" trend="V3 / V5 可选" hint="配置页明确版本" />
    <MetricCard class="span-3" label="停用应用" :value="disabledCount" trend="即时生效" hint="保留审计" />

    <GlassPanel
      class="span-12"
      title="接入应用"
      subtitle="每个应用在 HTTP 与 MQTT 之间单选 · 协议内按需启用入站和出站"
    >
      <template #action>
        <el-button v-if="auth.isAdmin" type="primary" @click="createDialogVisible = true">新建接入</el-button>
      </template>
      <el-table :data="applications" height="286">
        <el-table-column prop="name" label="应用名称" min-width="170">
          <template #default="{ row }">
            <div class="app-name">
              <b>{{ row.name }}</b
              ><small class="code">{{ row.integration_id }}</small>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="接入方式" width="120">
          <template #default="{ row }"
            ><el-tag effect="plain">{{ row.transport.toUpperCase() }}</el-tag></template
          >
        </el-table-column>
        <el-table-column label="调用方向" min-width="190">
          <template #default="{ row }">
            <div class="direction-tags">
              <el-tag v-for="direction in directions(row)" :key="direction" effect="plain">{{
                direction
              }}</el-tag>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="鉴权方式" min-width="180">
          <template #default="{ row }">{{ row.transport === "http" ? "Access Key + HMAC" : "Broker ACL" }}</template>
        </el-table-column>
        <el-table-column label="状态" width="168">
          <template #default="{ row }">
            <div class="runtime-toggle">
              <el-switch
                :model-value="row.enabled"
                :disabled="!auth.isAdmin || mqttRuntimeUnavailable(row)"
                @change="toggleIntegration(row, Boolean($event))"
              />
              <small v-if="mqttRuntimeUnavailable(row)">先启用 MQTT Runtime</small>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="应用有效期" width="150">
          <template #default="{ row }">{{ expiration(row.expires_at_ms) }}</template>
        </el-table-column>
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button link @click="openConfig(row)">配置</el-button>
            <el-button v-if="auth.isAdmin" link @click="openCredential(row)">凭证</el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel
      class="span-8"
      title="凭证管理"
      subtitle="secret 仅在创建或轮换时显示一次 · 查询始终脱敏"
    >
      <el-table :data="credentials" height="236">
        <el-table-column prop="integrationName" label="接入应用" min-width="150" />
        <el-table-column label="用途" min-width="190">
          <template #default="{ row }">{{ purposeLabel(row.purpose) }}</template>
        </el-table-column>
        <el-table-column label="Access Key" min-width="180">
          <template #default="{ row }"
            ><span class="code">{{ maskAccessKey(row.access_key) }}</span></template
          >
        </el-table-column>
        <el-table-column label="状态" width="96">
          <template #default="{ row }"
            ><StatusPill :label="row.status === 'active' ? '有效' : '已吊销'" :tone="row.status === 'active' ? 'ready' : 'warning'"
          /></template>
        </el-table-column>
        <el-table-column label="操作" width="90">
          <template #default="{ row }"
            ><el-button link :disabled="!auth.isAdmin || row.status !== 'active'" @click="revokeCredential(row)">吊销</el-button></template
          >
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-4" title="接入原则" subtitle="面向边端部署的最小可靠闭环">
      <div class="principle-list">
        <div>
          <span>01</span>
          <p><b>协议单选</b><small>同一应用选择 HTTP 或 MQTT</small></p>
        </div>
        <div>
          <span>02</span>
          <p><b>双向隔离</b><small>入站验证与出站签名凭证分离</small></p>
        </div>
        <div>
          <span>03</span>
          <p><b>有限持久化</b><small>只保留恢复、幂等和短期追踪所需数据</small></p>
        </div>
        <div>
          <span>04</span>
          <p><b>契约随服务发布</b><small>OpenAPI / AsyncAPI 由 Guard Server 提供</small></p>
        </div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-12" title="投递队列" subtitle="成功记录及时清理；重试与 DEAD 仅保留短期失败摘要">
      <template #action><el-button @click="loadData">刷新</el-button></template>
      <el-table :data="outbox" height="230" empty-text="当前没有待处理或失败投递">
        <el-table-column prop="integration_id" label="应用 ID" min-width="210" />
        <el-table-column prop="destination_kind" label="通道" width="110" />
        <el-table-column label="状态" width="120">
          <template #default="{ row }"><StatusPill :label="outboxState(row.state)" :tone="row.state === 'dead' ? 'warning' : 'info'" /></template>
        </el-table-column>
        <el-table-column prop="attempts" label="尝试" width="80" />
        <el-table-column prop="last_error" label="失败摘要" min-width="260" show-overflow-tooltip />
        <el-table-column label="更新时间" width="180">
          <template #default="{ row }">{{ new Date(row.updated_at_ms).toLocaleString() }}</template>
        </el-table-column>
        <el-table-column label="操作" width="100">
          <template #default="{ row }">
            <el-button link :disabled="row.state !== 'dead' || auth.session?.role === 'viewer'" @click="retryDelivery(row)">重试</el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <el-dialog v-model="createDialogVisible" title="新建三方接入" width="560px">
      <el-form label-position="top">
        <el-form-item label="应用名称"><el-input v-model="createForm.name" maxlength="255" /></el-form-item>
        <el-form-item label="接入方式">
          <el-radio-group v-model="createForm.transport">
            <el-radio-button value="http">HTTP</el-radio-button>
            <el-radio-button value="mqtt">MQTT</el-radio-button>
          </el-radio-group>
        </el-form-item>
        <el-form-item label="调用方向">
          <el-checkbox v-model="createForm.inbound_enabled">第三方调用 Guard</el-checkbox>
          <el-checkbox v-model="createForm.outbound_enabled">Guard 推送第三方</el-checkbox>
        </el-form-item>
        <el-form-item label="业务权限 Scope（逗号分隔）">
          <el-input v-model="scopeText" placeholder="devices:read, streams:write, events:read" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="createDialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="submitting" @click="submitIntegration">创建</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="credentialDialogVisible" title="创建 HMAC 凭证" width="520px">
      <el-form label-position="top">
        <el-form-item label="凭证用途">
          <el-radio-group v-model="credentialPurpose">
            <el-radio value="http_inbound_verify">调用 Guard API 验签</el-radio>
            <el-radio value="http_callback_sign">Guard 回调签名</el-radio>
          </el-radio-group>
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="credentialDialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="submitting" @click="submitCredential">创建凭证</el-button>
      </template>
    </el-dialog>

    <el-dialog v-model="secretDialogVisible" title="请立即保存 Secret" width="600px" :close-on-click-modal="false">
      <el-alert type="warning" :closable="false" title="Secret 只显示这一次，关闭后无法再次查询。" />
      <div class="secret-result"><span>Access Key</span><code>{{ createdAccessKey }}</code><span>Secret</span><code>{{ createdSecret }}</code></div>
      <template #footer><el-button type="primary" @click="secretDialogVisible = false">我已安全保存</el-button></template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from "vue";
import { useRouter } from "vue-router";
import { useAuthStore } from "@/stores/auth";
import { ElMessage, ElMessageBox } from "element-plus";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";
import {
  createIntegration,
  createIntegrationCredential,
  errorMessage,
  getIntegrationMqttRuntime,
  listIntegrationCredentials,
  listIntegrations,
  listOutbox,
  revokeIntegrationCredential,
  retryOutbox,
  updateIntegration,
  type IntegrationCredentialInfo,
  type IntegrationInfo,
  type IntegrationMqttRuntime,
  type OutboxInfo,
} from "@/api/client";

type CredentialRow = IntegrationCredentialInfo & { integrationName: string };
const router = useRouter();
const auth = useAuthStore();
const applications = ref<IntegrationInfo[]>([]);
const credentials = ref<CredentialRow[]>([]);
const outbox = ref<OutboxInfo[]>([]);
const mqttRuntime = ref<IntegrationMqttRuntime | null>(null);
const createDialogVisible = ref(false);
const credentialDialogVisible = ref(false);
const secretDialogVisible = ref(false);
const submitting = ref(false);
const selectedIntegration = ref<IntegrationInfo | null>(null);
const credentialPurpose = ref<IntegrationCredentialInfo["purpose"]>("http_inbound_verify");
const createdAccessKey = ref("");
const createdSecret = ref("");
const scopeText = ref("*");
const createForm = reactive({ name: "", transport: "http" as "http" | "mqtt", inbound_enabled: true, outbound_enabled: true });
const activeCredentialCount = computed(() => credentials.value.filter((item) => item.status === "active").length);
const mqttCount = computed(() => applications.value.filter((item) => item.transport === "mqtt").length);
const disabledCount = computed(() => applications.value.filter((item) => !item.enabled).length);

async function loadData() {
  try {
    const [integrations, runtime, records] = await Promise.all([
      listIntegrations(),
      getIntegrationMqttRuntime(),
      listOutbox(100),
    ]);
    applications.value = integrations;
    mqttRuntime.value = runtime;
    outbox.value = records;
    const httpApps = applications.value.filter((item) => item.transport === "http");
    const groups = await Promise.all(httpApps.map(async (item) => (await listIntegrationCredentials(item.integration_id)).map((credential) => ({ ...credential, integrationName: item.name }))));
    credentials.value = groups.flat();
  } catch (error) {
    ElMessage.error(errorMessage(error, "加载三方接入失败"));
  }
}

function mqttRuntimeUnavailable(row: IntegrationInfo) {
  return row.transport === "mqtt" && !mqttRuntime.value?.enabled;
}

function outboxState(value: OutboxInfo["state"]) {
  return { pending: "待投递", sending: "投递中", retry_wait: "等待重试", dead: "失败", delivered: "已完成" }[value];
}

async function retryDelivery(row: OutboxInfo) {
  try {
    await retryOutbox(row.outbox_id);
    await loadData();
    ElMessage.success("投递任务已重新入队");
  } catch (error) {
    ElMessage.error(errorMessage(error, "重新投递失败"));
  }
}

function directions(row: IntegrationInfo) {
  return [row.inbound_enabled ? "第三方调用" : "", row.outbound_enabled ? "事件推送" : ""].filter(Boolean);
}

function expiration(value: number | null) {
  return value ? new Date(value).toLocaleDateString() : "长期有效";
}

function purposeLabel(value: IntegrationCredentialInfo["purpose"]) {
  return value === "http_inbound_verify" ? "HTTP 请求验签" : "HTTP 回调签名";
}

function maskAccessKey(value: string) {
  return value.length > 10 ? `${value.slice(0, 7)}••••${value.slice(-4)}` : value;
}

function openConfig(row: IntegrationInfo) {
  void router.push(row.transport === "mqtt" ? "/integrations/mqtt" : "/integrations/http");
}

function openCredential(row: IntegrationInfo) {
  if (row.transport !== "http") {
    ElMessage.info("MQTT 使用 Broker TLS、账号与 Topic ACL，不创建 HMAC 凭证");
    return;
  }
  selectedIntegration.value = row;
  credentialDialogVisible.value = true;
}

async function submitIntegration() {
  if (!createForm.name.trim() || (!createForm.inbound_enabled && !createForm.outbound_enabled)) {
    ElMessage.warning("请填写应用名称并至少选择一个调用方向");
    return;
  }
  submitting.value = true;
  try {
    await createIntegration({ ...createForm, name: createForm.name.trim(), enabled: createForm.transport === "http", scopes: scopeText.value.split(",").map((item) => item.trim()).filter(Boolean), expires_at_ms: null });
    createDialogVisible.value = false;
    createForm.name = "";
    await loadData();
    ElMessage.success("三方接入已创建");
  } catch (error) {
    ElMessage.error(errorMessage(error, "创建三方接入失败"));
  } finally {
    submitting.value = false;
  }
}

async function submitCredential() {
  if (!selectedIntegration.value) return;
  submitting.value = true;
  try {
    const result = await createIntegrationCredential(selectedIntegration.value.integration_id, credentialPurpose.value, null);
    createdAccessKey.value = result.credential.access_key;
    createdSecret.value = result.secret;
    credentialDialogVisible.value = false;
    secretDialogVisible.value = true;
    await loadData();
  } catch (error) {
    ElMessage.error(errorMessage(error, "创建 HMAC 凭证失败"));
  } finally {
    submitting.value = false;
  }
}

async function revokeCredential(row: CredentialRow) {
  try {
    await ElMessageBox.confirm(`确认吊销 ${row.integrationName} 的这条凭证？`, "吊销凭证", { type: "warning" });
    await revokeIntegrationCredential(row.integration_id, row.credential_id);
    await loadData();
    ElMessage.success("凭证已吊销");
  } catch (error) {
    if (error !== "cancel" && error !== "close") ElMessage.error(errorMessage(error, "吊销凭证失败"));
  }
}

async function toggleIntegration(row: IntegrationInfo, enabled: boolean) {
  if (enabled && mqttRuntimeUnavailable(row)) {
    ElMessage.warning("请先在 Guard 配置中启用 MQTT Runtime 并重启服务");
    return;
  }
  try {
    await updateIntegration(row.integration_id, {
      name: row.name,
      inbound_enabled: row.inbound_enabled,
      outbound_enabled: row.outbound_enabled,
      enabled,
      scopes: row.scopes,
      expires_at_ms: row.expires_at_ms,
      expected_config_version: row.config_version,
    });
    await loadData();
    ElMessage.success(enabled ? "接入应用已启用" : "接入应用已停用");
  } catch (error) {
    ElMessage.error(errorMessage(error, "更新接入应用状态失败"));
  }
}

onMounted(loadData);
</script>

<style scoped>
.app-name {
  display: grid;
  gap: 5px;
}

.app-name small {
  color: var(--faint);
  font-size: 11px;
}

.direction-tags {
  display: flex;
  gap: 6px;
  flex-wrap: wrap;
}

.runtime-toggle {
  display: grid;
  gap: 3px;
  justify-items: start;
}

.runtime-toggle small {
  color: var(--yellow);
  white-space: nowrap;
}

.principle-list {
  display: grid;
  gap: 10px;
}

.principle-list > div {
  display: grid;
  grid-template-columns: 34px minmax(0, 1fr);
  gap: 12px;
  align-items: center;
  padding: 11px 12px;
  border: 1px solid rgba(37, 146, 255, 0.24);
  border-radius: 13px;
  background: rgba(4, 16, 47, 0.48);
}

.principle-list > div > span {
  color: var(--cyan);
  font-family: "JetBrains Mono", Consolas, monospace;
  font-size: 12px;
  font-weight: 800;
}

.principle-list p {
  display: grid;
  gap: 4px;
  margin: 0;
}

.principle-list small {
  color: var(--muted);
  line-height: 1.4;
}

.secret-result {
  display: grid;
  grid-template-columns: 100px minmax(0, 1fr);
  gap: 12px;
  margin-top: 18px;
  align-items: center;
}

.secret-result span {
  color: var(--muted);
}

.secret-result code {
  padding: 10px 12px;
  border: 1px solid rgba(37, 146, 255, 0.28);
  border-radius: 9px;
  color: #c8f3ff;
  overflow-wrap: anywhere;
}
</style>
