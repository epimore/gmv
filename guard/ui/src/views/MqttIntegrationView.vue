<template>
  <div class="page-grid">
    <MetricCard
      class="span-3"
      label="接入方向"
      value="双向"
      trend="Subscribe / Publish"
      hint="按需启用"
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
          <div class="flow-meta"><span>事件主题</span><code>gmv/events/{event_type}</code></div>
        </article>
      </div>
    </GlassPanel>

    <GlassPanel class="span-4" title="Broker 参数预览" subtitle="连接信息与业务凭证分用途管理">
      <div class="broker-status">
        <span class="pulse" />
        <div><b>连接配置完整</b><small>演示状态 · 尚未连接真实 Broker</small></div>
      </div>
      <div class="kv">
        <div class="kv-item wide">
          <span>Broker</span><b class="code">mqtts://broker.example.com:8883</b>
        </div>
        <div class="kv-item"><span>Client ID</span><b class="code">gmv-guard-edge-01</b></div>
        <div class="kv-item"><span>Keep Alive</span><b>30 s</b></div>
        <div class="kv-item"><span>连接鉴权</span><b>TLS + Username</b></div>
        <div class="kv-item"><span>消息签名</span><b>待策略确认</b></div>
      </div>
    </GlassPanel>

    <GlassPanel
      class="span-12"
      title="Topic 契约预览"
      subtitle="以 Guard 为观察方描述 publish / subscribe 方向"
    >
      <template #action><el-button @click="showPlaceholder">查看在线文档</el-button></template>
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
          ><el-tag effect="plain">内置静态页</el-tag>
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
import { ElMessage } from "element-plus";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";

const channels = [
  {
    operation: "SUBSCRIBE",
    topic: "gmv/commands/{integration_id}",
    message: "IntegrationCommand",
    qos: "1",
    auth: "Broker ACL",
    tracking: "command_id",
  },
  {
    operation: "PUBLISH",
    topic: "gmv/command-results/{integration_id}",
    message: "CommandResult",
    qos: "1",
    auth: "Broker ACL",
    tracking: "command_id / operation_id",
  },
  {
    operation: "PUBLISH",
    topic: "gmv/events/{event_type}",
    message: "EventEnvelope",
    qos: "1",
    auth: "Topic mapping",
    tracking: "event_id / trace_id",
  },
  {
    operation: "PUBLISH",
    topic: "gmv/status/{integration_id}",
    message: "IntegrationStatus",
    qos: "1",
    auth: "Topic mapping",
    tracking: "integration_id",
  },
];

function showPlaceholder() {
  ElMessage.info("AsyncAPI 在线文档将在 Guard Server 契约生成能力完成后开放");
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
</style>
