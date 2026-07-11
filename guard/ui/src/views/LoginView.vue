<template>
  <div>
    <main class="login-page">
      <section class="login-hero">
        <div class="login-visual-stage">
          <img class="login-visual-image" src="/log.png" alt="GMV 音视频平台" />
          <div class="login-benefits" aria-label="GMV 平台能力">
            <div v-for="item in featureCards" :key="item.title" class="login-benefit">
              <div class="login-benefit-icon"><el-icon>
                  <component :is="item.icon" />
                </el-icon></div>
              <div><b>{{ item.title }}</b><span>{{ item.detail }}</span></div>
            </div>
          </div>
        </div>
      </section>

      <section class="login-card">
        <div class="brand">
          <img class="brand-mark" src="/favicon.ico" alt="" />
          <div>
            <div class="brand-title">GMV</div>
            <div class="brand-sub">视音频监控，集成AI能力，实时感知，精准分析</div>
          </div>
        </div>
        <div class="login-card-head">
          <h2>登录</h2>
          <p>进入（测试/演示）控制台</p>
        </div>
        <el-form label-position="top" @submit.prevent="submit">
          <el-form-item label="用户名">
            <el-input v-model="form.username" size="large" placeholder="请输入用户名" :prefix-icon="User" />
          </el-form-item>
          <el-form-item label="密码">
            <el-input v-model="form.password" size="large" type="password" placeholder="请输入密码" :prefix-icon="Lock"
              show-password />
          </el-form-item>
          <div class="login-form-meta">
            <el-checkbox v-model="form.remember">记住此设备</el-checkbox>
            <span class="code">REST API v2</span>
          </div>
          <el-button class="login-submit" type="primary" native-type="submit" size="large" :loading="loading">
            安全登录 <el-icon>
              <ArrowRight />
            </el-icon>
          </el-button>
        </el-form>
      </section>
    </main>

    <footer class="footer">
      <p>[这是一个开源的项目:<a href="https://github.com/epimore/gmv"
          target="_blank">https://github.com/epimore/gmv</a>，仅供开发、测试、学习、交流使用];[微信交流添加：epimore;备注GMV]；[ICP备案/许可证号：蜀ICP备2023006262号-1]
      </p>
    </footer>
  </div>

</template>

<script setup lang="ts">
import { reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { ArrowRight, Box, Cloudy, Connection, Cpu, Grid, Lock, User } from '@element-plus/icons-vue';
import { useRouter } from 'vue-router';
import { errorMessage } from '@/api/client';
import { useAuthStore } from '@/stores/auth';

const router = useRouter();
const auth = useAuthStore();
const loading = ref(false);
const form = reactive({ username: '', password: '', remember: true });
const featureCards = [
  { icon: Box, title: '可 · 开箱即用', detail: '快速部署，即装即用' },
  { icon: Connection, title: '易 · 三方集成', detail: '开放接口，灵活对接' },
  { icon: Cpu, title: '低 · 硬件资源', detail: '低配运行，高效利用' },
  { icon: Grid, title: '简 · 集群扩展', detail: '弹性扩容，简易运维' },
  { icon: Cloudy, title: '能 · 云边协同', detail: '云边联动，高效协同' },
];

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
