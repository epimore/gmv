<template>
  <main class="login-page">
    <section class="login-card">
      <div class="brand">
        <div class="brand-mark">G</div>
        <div>
          <div class="brand-title">GMV</div>
          <div class="brand-sub">Control Plane</div>
        </div>
      </div>
      <h1>登录</h1>
      <p>进入 GMV 控制台，管理节点、流媒体、智能分析与系统集成。</p>
      <el-form label-position="top">
        <el-form-item label="用户名">
          <el-input v-model="form.username" size="large" placeholder="请输入用户名" />
        </el-form-item>
        <el-form-item label="密码">
          <el-input v-model="form.password" size="large" type="password" placeholder="请输入密码" show-password />
        </el-form-item>
        <div class="toolbar" style="justify-content: space-between; margin: 4px 0 22px;">
          <el-checkbox v-model="form.remember">记住此设备</el-checkbox>
          <span class="code">REST API v2</span>
        </div>
        <el-button type="primary" size="large" style="width: 100%;" :loading="loading" @click="submit">安全登录</el-button>
      </el-form>
      <div class="kv" style="margin-top: 22px;">
        <div class="kv-item"><span>TLS</span><b>默认启用</b></div>
        <div class="kv-item"><span>NTP</span><b>已校验</b></div>
        <div class="kv-item"><span>模式</span><b>部署级</b></div>
        <div class="kv-item"><span>审计</span><b>写操作记录</b></div>
      </div>
    </section>
    <section class="login-visual">
      <div class="preview-card">
        <div class="panel-title">星舰控制面</div>
        <div class="panel-kicker">节点、租约、事件通过 REST polling 汇聚</div>
        <OrbitChart :option="lineOption('接入延迟')" sm />
      </div>
      <div class="preview-card bottom">
        <div class="panel-title">安全基线</div>
        <div class="kv" style="margin-top: 12px;">
          <div class="kv-item"><span>CSRF</span><b>启用</b></div>
          <div class="kv-item"><span>Origin</span><b>校验</b></div>
        </div>
      </div>
    </section>
  </main>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { useRouter } from 'vue-router';
import { liveApi, login } from '@/api/client';
import OrbitChart from '@/components/OrbitChart.vue';
import { lineOption } from '@/data/mock';

const router = useRouter();
const loading = ref(false);
const form = reactive({ username: '', password: '', remember: true });

async function submit() {
  loading.value = true;
  try {
    if (liveApi) await login(form.username, form.password);
    await router.push('/dashboard');
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '登录失败');
  } finally {
    loading.value = false;
  }
}
</script>
