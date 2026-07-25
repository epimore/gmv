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

    <GlassPanel class="span-4" title="接入参数预览" subtitle="最终配置由 Guard Server 提供">
      <div class="kv">
        <div class="kv-item"><span>开放 API</span><b class="code">/openapi/v1</b></div>
        <div class="kv-item"><span>协议</span><b>HTTPS</b></div>
        <div class="kv-item"><span>签名算法</span><b>HMAC-SHA256</b></div>
        <div class="kv-item"><span>幂等字段</span><b class="code">request_id</b></div>
        <div class="kv-item wide">
          <span>回调地址</span><b class="code">https://partner.example.com/gmv/events</b>
        </div>
        <div class="kv-item wide"><span>凭证用途</span><b>入站验签 / 回调签名分离</b></div>
      </div>
    </GlassPanel>

    <GlassPanel
      class="span-12"
      title="接口契约预览"
      subtitle="示例结构 · 正式内容由代码生成的 OpenAPI 提供"
    >
      <template #action><el-button @click="showPlaceholder">查看在线文档</el-button></template>
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
import { ElMessage } from "element-plus";
import GlassPanel from "@/components/GlassPanel.vue";
import MetricCard from "@/components/MetricCard.vue";
import StatusPill from "@/components/StatusPill.vue";

const contracts = [
  {
    direction: "被调用",
    method: "GET",
    path: "/openapi/v1/devices",
    purpose: "查询设备与通道",
    auth: "Access Key + HMAC",
    tracking: "trace_id",
  },
  {
    direction: "被调用",
    method: "POST",
    path: "/openapi/v1/streams/preview",
    purpose: "创建实时预览",
    auth: "Access Key + HMAC",
    tracking: "request_id / operation_id",
  },
  {
    direction: "被调用",
    method: "POST",
    path: "/openapi/v1/streams/stop",
    purpose: "停止业务流",
    auth: "Access Key + HMAC",
    tracking: "request_id / operation_id",
  },
  {
    direction: "回调",
    method: "POST",
    path: "{callback_url}/events",
    purpose: "推送事件与操作结果",
    auth: "Guard 回调签名",
    tracking: "event_id / trace_id",
  },
];

function showPlaceholder() {
  ElMessage.info("OpenAPI 在线文档将在 Guard Server 契约生成能力完成后开放");
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
