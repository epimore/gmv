<template>
  <div class="page-grid">
    <MetricCard class="span-3" label="接入应用" value="2" trend="HTTP / MQTT" hint="方式单选" />
    <MetricCard class="span-3" label="有效凭证" value="3" trend="分方向授权" hint="HMAC / Broker" />
    <MetricCard class="span-3" label="即将到期" value="1" trend="30 天内" hint="支持轮换" />
    <MetricCard class="span-3" label="异常配置" value="0" trend="配置完整" hint="当前预览" />

    <GlassPanel
      class="span-12"
      title="接入应用"
      subtitle="每个应用在 HTTP 与 MQTT 之间单选 · 协议内按需启用入站和出站"
    >
      <template #action>
        <el-button type="primary" @click="showPlaceholder('新建接入')">新建接入</el-button>
      </template>
      <el-table :data="applications" height="286">
        <el-table-column prop="name" label="应用名称" min-width="170">
          <template #default="{ row }">
            <div class="app-name">
              <b>{{ row.name }}</b
              ><small class="code">{{ row.id }}</small>
            </div>
          </template>
        </el-table-column>
        <el-table-column label="接入方式" width="120">
          <template #default="{ row }"
            ><el-tag effect="plain">{{ row.transport }}</el-tag></template
          >
        </el-table-column>
        <el-table-column label="调用方向" min-width="190">
          <template #default="{ row }">
            <div class="direction-tags">
              <el-tag v-for="direction in row.directions" :key="direction" effect="plain">{{
                direction
              }}</el-tag>
            </div>
          </template>
        </el-table-column>
        <el-table-column prop="auth" label="鉴权方式" min-width="180" />
        <el-table-column label="状态" width="100">
          <template #default="{ row }"
            ><StatusPill
              :label="row.enabled ? '启用' : '停用'"
              :tone="row.enabled ? 'ready' : 'danger'"
          /></template>
        </el-table-column>
        <el-table-column prop="expiresAt" label="凭证有效期" width="150" />
        <el-table-column label="操作" width="150" fixed="right">
          <template #default="{ row }">
            <el-button link @click="showPlaceholder(`配置 ${row.name}`)">配置</el-button>
            <el-button link @click="showPlaceholder(`轮换 ${row.name} 凭证`)">轮换</el-button>
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
        <el-table-column prop="name" label="凭证名称" min-width="150" />
        <el-table-column prop="purpose" label="用途" min-width="190" />
        <el-table-column label="Access Key" min-width="180">
          <template #default="{ row }"
            ><span class="code">{{ row.accessKey }}</span></template
          >
        </el-table-column>
        <el-table-column label="状态" width="96">
          <template #default="{ row }"
            ><StatusPill :label="row.status" :tone="row.status === '有效' ? 'ready' : 'warning'"
          /></template>
        </el-table-column>
        <el-table-column label="操作" width="90">
          <template #default="{ row }"
            ><el-button link @click="showPlaceholder(`管理 ${row.name}`)">管理</el-button></template
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
  </div>
</template>

<script setup lang="ts">
import { ElMessage } from "element-plus";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";

const applications = [
  {
    id: "partner-http-01",
    name: "行业管理平台",
    transport: "HTTP",
    directions: ["第三方调用", "事件回调"],
    auth: "Access Key + HMAC",
    enabled: true,
    expiresAt: "2027-07-01",
  },
  {
    id: "partner-mqtt-01",
    name: "告警联动中心",
    transport: "MQTT",
    directions: ["命令订阅", "事件发布"],
    auth: "TLS + Broker ACL",
    enabled: true,
    expiresAt: "长期有效",
  },
];

const credentials = [
  { name: "行业平台入站", purpose: "HTTP 请求验签", accessKey: "ak_http_••••7D2A", status: "有效" },
  {
    name: "行业平台回调",
    purpose: "HTTP 回调签名",
    accessKey: "ak_callback_••••9F10",
    status: "即将到期",
  },
  {
    name: "告警中心连接",
    purpose: "MQTT Broker 连接",
    accessKey: "client_gmv_••••01",
    status: "有效",
  },
];

function showPlaceholder(action: string) {
  ElMessage.info(`${action}功能将在服务端接入能力完成后开放`);
}
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
</style>
