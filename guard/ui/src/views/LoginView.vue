<template>
  <main class="login-page">
    <section class="login-card">
      <div class="brand">
        <img class="brand-mark" src="/favicon.ico" alt="" />
        <div>
          <div class="brand-title">GMV</div>
          <div class="brand-sub">一个可定制的AI智能视音频监控中心</div>
        </div>
      </div>
      <h1>登录</h1>
      <p>进入 GMV 控制台（测试/演示）。</p>
      <el-form label-position="top" @submit.prevent="submit">
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
        <el-button type="primary" native-type="submit" size="large" style="width: 100%;"
          :loading="loading">安全登录</el-button>
      </el-form>
      <!-- <div class="kv" style="margin-top: 22px;">
        <div class="kv-item"><span>GB28181</span><b>兼容2016/2022</b></div>
        <div class="kv-item"><span>ONVIF</span><b>开发中</b></div>
        <div class="kv-item"><span>流媒体</span><b>H265/H264/AAC/G711</b></div>
        <div class="kv-item"><span>审计</span><b>写操作记录</b></div>
      </div> -->
    </section>
    <section class="login-visual">
      <img class="login-visual-image" src="/log.png" alt="GMV 音视频平台" />
    </section>
  </main>
</template>

<script setup lang="ts">
import { reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { useRouter } from 'vue-router';
import { errorMessage } from '@/api/client';
import { useAuthStore } from '@/stores/auth';

const router = useRouter();
const auth = useAuthStore();
const loading = ref(false);
const form = reactive({ username: '', password: '', remember: true });

async function submit() {
  if (loading.value) return;
  if (!form.username.trim() || !form.password) {
    ElMessage.warning('请输入用户名和密码');
    return;
  }
  loading.value = true;
  try {
    await auth.signIn(form.username.trim(), form.password);
    const redirect = typeof router.currentRoute.value.query.redirect === 'string'
      ? router.currentRoute.value.query.redirect
      : '/dashboard';
    await router.push(redirect);
  } catch (error) {
    ElMessage.error(errorMessage(error, '登录失败'));
  } finally {
    loading.value = false;
  }
}
</script>
