<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="接入状态" :value="integrationStatusLabel" trend="业务应用" :hint="integrationStatusHint" />
    <MetricCard class="span-3" label="配置方式" :value="transportLabel" trend="HTTP / MQTT" :hint="app?.enabled ? '已保存' : '未配置'" />
    <MetricCard class="span-3" label="应用开关" :value="app?.enabled ? '已启用' : '未启用'" trend="服务端状态" :hint="app?.integration_id ?? '尚未创建'" />
    <MetricCard class="span-3" label="配置方向" :value="directionLabel" trend="Inbound / Outbound" hint="保存值" />

    <GlassPanel class="span-8" title="接入应用" subtitle="只维护一个第三方业务应用；HTTP 与 MQTT 单选，也可以保持未集成。">
      <el-form label-position="top" class="app-form" :disabled="!canManage">
        <el-form-item label="应用名称">
          <el-input v-model="form.name" placeholder="例如：园区业务平台" maxlength="128" />
        </el-form-item>
        <el-form-item label="接入方式">
          <el-radio-group v-model="transportChoice" @change="handleTransportChange">
            <el-radio-button value="">未集成</el-radio-button>
            <el-radio-button value="http">HTTP</el-radio-button>
            <el-radio-button value="mqtt">MQTT</el-radio-button>
          </el-radio-group>
          <div class="field-help">
            当前接入：{{ transportLabel }}。
            <template v-if="hasTransportDraft">
              本次选择：{{ selectedTransportLabel }}（尚未保存）。
              保存并启用应用后，将进入对应子页面维护详细配置。
            </template>
            <template v-else-if="!app?.enabled">请选择 HTTP 或 MQTT，启用应用并保存。</template>
          </div>
        </el-form-item>
        <div class="form-grid">
          <el-form-item label="接收入站命令">
            <el-switch v-model="form.inbound_enabled" :disabled="!transportChoice" />
          </el-form-item>
          <el-form-item label="发送回调 / 事件">
            <el-switch v-model="form.outbound_enabled" :disabled="!transportChoice" />
          </el-form-item>
          <el-form-item label="启用应用">
            <el-switch
              v-model="form.enabled"
              :disabled="!transportChoice"
            />
            <div class="field-help">
              当前已保存：{{ app?.enabled ? '已启用' : '未启用' }}。
              <template v-if="form.enabled !== (app?.enabled ?? false)">本次修改为{{ form.enabled ? '启用' : '停用' }}，尚未保存。</template>
              <template v-else-if="transportChoice === 'mqtt' && !mqttRuntime?.configured">应用保存后将进入 MQTT 接入页面维护 Broker Runtime。</template>
            </div>
          </el-form-item>
        </div>
        <el-form-item label="授权范围">
          <el-select v-model="form.scopes" multiple filterable collapse-tags placeholder="选择业务权限" :disabled="!transportChoice">
            <el-option v-for="scope in scopeOptions" :key="scope" :label="scope" :value="scope" />
          </el-select>
        </el-form-item>
        <div class="form-actions">
          <el-button type="primary" :loading="saving" :disabled="!canSave" @click="save">
            {{ saveButtonLabel }}
          </el-button>
        </div>
      </el-form>
    </GlassPanel>

    <GlassPanel class="span-4" title="后续配置" subtitle="接入参数在对应子页面维护。">
      <div class="next-steps">
        <el-alert v-if="!transportChoice" title="尚未选择接入方式" description="选择 HTTP 或 MQTT，启用并保存应用后，将进入对应配置页面。" type="info" :closable="false" show-icon />
        <template v-else-if="transportChoice === 'http'">
          <el-alert v-if="!activeTransportSelected" title="请先保存并启用 HTTP 应用" description="保存后系统会生成业务应用 ID 和 HTTP 默认配置，并进入 HTTP 接入页面。" type="info" :closable="false" show-icon />
          <p>HTTP 接入页面维护 HMAC 凭证、回调策略和事件映射。</p>
          <el-button type="primary" plain :disabled="!activeTransportSelected" @click="$router.push('/integrations/http')">进入 HTTP 接入</el-button>
        </template>
        <template v-else>
          <el-alert
            v-if="!activeTransportSelected"
            title="请先保存并启用 MQTT 应用"
            description="保存后系统会生成业务应用 ID 和只读 Topic 默认配置，并进入 MQTT 接入页面。"
            type="info"
            :closable="false"
            show-icon
          />
          <el-alert
            v-else-if="!mqttRuntime?.configured"
            title="MQTT Runtime 待配置"
            description="业务应用已经启用；请进入 MQTT 接入页面保存 Broker、协议版本和账号配置，保存后自动生效，无需返回再次启用。"
            type="warning"
            :closable="false"
            show-icon
          />
          <p>MQTT 接入页面维护 Broker Runtime、协议版本、账号、Topic 契约和可靠投递信息。</p>
          <el-button type="primary" plain :disabled="!activeTransportSelected" @click="$router.push('/integrations/mqtt')">进入 MQTT 接入</el-button>
        </template>
        <el-divider />
        <div class="identity-row"><span>Integration ID</span><code>{{ app?.integration_id ?? '保存后生成' }}</code></div>
        <div class="identity-row"><span>配置版本</span><strong>{{ app?.config_version ?? 0 }}</strong></div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-12" title="集成主密钥" subtitle="由 Guard 首次启动时随机生成并保存在数据库中；页面不会显示或接收密钥明文。">
      <div class="master-key-row">
        <div class="master-key-status">
          <el-tag :type="masterKey?.configured ? 'success' : 'danger'">
            {{ masterKey?.configured ? '已初始化' : '不可用' }}
          </el-tag>
          <span>版本 {{ masterKey?.key_version ?? '-' }}</span>
          <span>最近更新 {{ formatTime(masterKey?.updated_at_ms) }}</span>
          <span>操作人 {{ masterKey?.updated_by ?? '-' }}</span>
        </div>
        <el-button type="warning" plain :loading="rotatingKey" :disabled="!canManage || !masterKey?.configured" @click="rotateMasterKey">
          轮换主密钥
        </el-button>
      </div>
      <el-alert class="master-key-alert" type="warning" :closable="false" show-icon title="轮换会在一个数据库事务内重新加密全部 HTTP 凭证与 MQTT 密码；操作期间相关密钥读写会短暂等待。" />
    </GlassPanel>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { useRouter } from 'vue-router';
import { ElMessage, ElMessageBox } from 'element-plus';
import GlassPanel from '@/components/GlassPanel.vue';
import MetricCard from '@/components/MetricCard.vue';
import { errorMessage, getBusinessIntegration, getIntegrationMasterKey, getIntegrationMqttRuntime, rotateIntegrationMasterKey, saveBusinessIntegration, type IntegrationInfo, type IntegrationMasterKeyState, type IntegrationMqttRuntime, type IntegrationTransport } from '@/api/client';
import { useAuthStore } from '@/stores/auth';

const auth = useAuthStore();
const router = useRouter();
const app = ref<IntegrationInfo | null>(null);
const transportChoice = ref<IntegrationTransport | ''>('');
const saving = ref(false);
const rotatingKey = ref(false);
const masterKey = ref<IntegrationMasterKeyState | null>(null);
const mqttRuntime = ref<IntegrationMqttRuntime | null>(null);
const form = reactive({ name: '', inbound_enabled: true, outbound_enabled: true, enabled: false, scopes: [] as string[] });
const scopeOptions = ['devices:read', 'devices:write', 'streams:read', 'streams:write', 'records:read', 'events:read', 'ai:read', 'ai:write'];
const canManage = computed(() => auth.session?.role === 'admin');
const integrationStatusLabel = computed(() => {
  if (!app.value?.enabled) return '未集成';
  if (app.value.transport === 'mqtt' && !mqttRuntime.value?.configured) return '待配置';
  return '已集成';
});
const integrationStatusHint = computed(() => {
  if (!app.value?.enabled) return '尚未配置';
  if (app.value.transport === 'mqtt' && !mqttRuntime.value?.configured) return 'MQTT Runtime 待配置';
  return '应用运行中';
});
const transportLabel = computed(() => !app.value?.enabled ? '未集成' : app.value.transport === 'http' ? 'HTTP' : 'MQTT');
const selectedTransportLabel = computed(() => transportChoice.value === 'http' ? 'HTTP' : transportChoice.value === 'mqtt' ? 'MQTT' : '未集成');
const hasTransportDraft = computed(() => transportChoice.value !== (app.value?.enabled ? app.value.transport : ''));
const activeTransportSelected = computed(() => Boolean(
  app.value?.enabled && transportChoice.value === app.value.transport,
));
const directionLabel = computed(() => !app.value?.enabled ? '未集成' : app.value.inbound_enabled && app.value.outbound_enabled ? '双向' : app.value.inbound_enabled ? '入站' : app.value.outbound_enabled ? '出站' : '未开启');
const saveButtonLabel = computed(() => {
  if (app.value?.enabled && !form.enabled && (!transportChoice.value || transportChoice.value === app.value.transport)) return '停用应用';
  if (transportChoice.value && form.enabled) return `保存并进入 ${selectedTransportLabel.value} 接入`;
  return '保存应用';
});
const hasChanges = computed(() => {
  const saved = app.value;
  if (!saved) return true;
  return transportChoice.value !== (saved.enabled ? saved.transport : '')
    || form.name.trim() !== saved.name
    || form.inbound_enabled !== saved.inbound_enabled
    || form.outbound_enabled !== saved.outbound_enabled
    || form.enabled !== saved.enabled
    || form.scopes.length !== saved.scopes.length
    || form.scopes.some((scope) => !saved.scopes.includes(scope));
});
const canSave = computed(() => {
  if (!hasChanges.value) return false;
  if (app.value?.enabled && !form.enabled) {
    return !transportChoice.value || transportChoice.value === app.value.transport;
  }
  return Boolean(form.name.trim() && transportChoice.value && form.enabled);
});

function requestId(): string { return crypto.randomUUID?.() ?? `${Date.now()}-${Math.random()}`; }

function fill(value: IntegrationInfo | null): void {
  app.value = value;
  transportChoice.value = value?.enabled ? value.transport : '';
  form.name = value?.name ?? '';
  form.inbound_enabled = value?.inbound_enabled ?? true;
  form.outbound_enabled = value?.outbound_enabled ?? true;
  form.enabled = value?.enabled ?? false;
  form.scopes = [...(value?.scopes ?? [])];
}

function handleTransportChange(): void {
  form.enabled = Boolean(
    transportChoice.value
    && app.value?.enabled
    && transportChoice.value === app.value.transport,
  );
}

async function load(): Promise<void> {
  try {
    const [response, key, runtime] = await Promise.all([
      getBusinessIntegration(),
      getIntegrationMasterKey(),
      getIntegrationMqttRuntime(),
    ]);
    fill(response.integration);
    masterKey.value = key;
    mqttRuntime.value = runtime;
  } catch (error) {
    ElMessage.error(errorMessage(error, '加载接入应用失败'));
  }
}

function formatTime(value?: number): string {
  return value ? new Date(value).toLocaleString() : '-';
}

async function rotateMasterKey(): Promise<void> {
  if (!masterKey.value) return;
  try {
    await ElMessageBox.confirm(
      '确认轮换集成主密钥？Guard 会重新加密现有 HTTP 凭证和 MQTT 密码，密钥明文不会返回页面。',
      '轮换集成主密钥',
      { confirmButtonText: '确认轮换', cancelButtonText: '取消', type: 'warning' },
    );
  } catch {
    return;
  }
  rotatingKey.value = true;
  try {
    masterKey.value = await rotateIntegrationMasterKey(requestId(), masterKey.value.key_version);
    ElMessage.success('集成主密钥已轮换');
  } catch (error) {
    ElMessage.error(errorMessage(error, '轮换集成主密钥失败'));
    masterKey.value = await getIntegrationMasterKey().catch(() => masterKey.value);
  } finally {
    rotatingKey.value = false;
  }
}

async function save(): Promise<void> {
  const current = app.value;
  if (!form.enabled) {
    if (!current?.enabled || (transportChoice.value && transportChoice.value !== current.transport)) return;
    saving.value = true;
    try {
      const disabled = await saveBusinessIntegration({
        request_id: requestId(),
        name: current.name,
        transport: current.transport,
        inbound_enabled: current.inbound_enabled,
        outbound_enabled: current.outbound_enabled,
        enabled: false,
        scopes: [...current.scopes],
        expires_at_ms: current.expires_at_ms,
        expected_config_version: current.config_version,
      });
      fill(disabled);
      ElMessage.success('第三方业务应用已停用');
    } catch (error) {
      await load().catch(() => undefined);
      ElMessage.error(errorMessage(error, '停用接入应用失败'));
    } finally {
      saving.value = false;
    }
    return;
  }
  if (!transportChoice.value) return;
  const targetTransport = transportChoice.value;
  const switchingTransport = Boolean(current?.enabled && current.transport !== targetTransport);
  if (switchingTransport && current) {
    const currentLabel = current.transport === 'http' ? 'HTTP' : 'MQTT';
    const targetLabel = targetTransport === 'http' ? 'HTTP' : 'MQTT';
    try {
      await ElMessageBox.confirm(
        `当前 ${currentLabel} 应用仍在运行。切换会先停用 ${currentLabel}，再启用 ${targetLabel}。`,
        '切换接入方式',
        { confirmButtonText: '确认切换', cancelButtonText: '取消', type: 'warning' },
      );
    } catch {
      return;
    }
  }

  saving.value = true;
  try {
    let expectedConfigVersion = current?.config_version ?? 0;
    if (switchingTransport && current) {
      const disabled = await saveBusinessIntegration({
        request_id: requestId(),
        name: current.name,
        transport: current.transport,
        inbound_enabled: current.inbound_enabled,
        outbound_enabled: current.outbound_enabled,
        enabled: false,
        scopes: [...current.scopes],
        expires_at_ms: current.expires_at_ms,
        expected_config_version: expectedConfigVersion,
      });
      expectedConfigVersion = disabled.config_version;
    }
    const saved = await saveBusinessIntegration({
      request_id: requestId(),
      name: form.name.trim(),
      transport: targetTransport,
      inbound_enabled: form.inbound_enabled,
      outbound_enabled: form.outbound_enabled,
      enabled: true,
      scopes: [...form.scopes],
      expires_at_ms: null,
      expected_config_version: expectedConfigVersion,
    });
    fill(saved);
    ElMessage.success(`接入应用已保存并启用，正在进入 ${targetTransport === 'http' ? 'HTTP' : 'MQTT'} 接入页面`);
  } catch (error) {
    await load().catch(() => undefined);
    ElMessage.error(errorMessage(error, '保存接入应用失败'));
    return;
  } finally {
    saving.value = false;
  }
  await router.push(`/integrations/${targetTransport}`);
}

onMounted(load);
</script>

<style scoped>
.app-form { max-width: 760px; }
.form-grid { display: grid; grid-template-columns: repeat(3, minmax(0, 1fr)); gap: 16px; }
.field-help { margin-top: 8px; color: var(--text-muted); font-size: 12px; }
.form-actions { display: flex; justify-content: flex-end; }
.next-steps { display: grid; gap: 14px; color: var(--text-secondary); }
.next-steps p { margin: 0; line-height: 1.7; }
.identity-row { display: flex; justify-content: space-between; gap: 12px; align-items: center; }
.identity-row code { overflow-wrap: anywhere; color: var(--accent-cyan); }
.master-key-row { display: flex; align-items: center; justify-content: space-between; gap: 16px; }
.master-key-status { display: flex; align-items: center; flex-wrap: wrap; gap: 16px; color: var(--text-secondary); }
.master-key-alert { margin-top: 16px; }
@media (max-width: 760px) { .form-grid { grid-template-columns: 1fr; gap: 0; } }
</style>
