<template>
  <div class="app-shell">
    <div class="console-texture" />
    <aside class="sidebar">
      <RouterLink class="brand" to="/dashboard" aria-label="GMV 总览">
        <div class="brand-mark">G</div>
        <div>
          <div class="brand-title">GMV</div>
          <div class="brand-sub">Control Plane</div>
        </div>
      </RouterLink>
      <nav>
        <template v-for="group in groups" :key="group">
          <div class="nav-group">{{ group }}</div>
          <template v-for="item in grouped[group]" :key="item.path">
            <RouterLink class="nav-item" :class="{ 'has-children': item.children?.length, 'is-section-active': item.children?.some((child) => route.path.startsWith(child.path)) }" :to="item.path">
              <span class="nav-icon">{{ item.icon }}</span>
              <span class="nav-label">{{ item.label }}</span>
            </RouterLink>
            <div v-if="item.children?.length" class="nav-children">
              <RouterLink v-for="child in item.children" :key="child.path" class="nav-item nav-child" :to="child.path">
                <span class="nav-icon">{{ child.icon }}</span>
                <span class="nav-label">{{ child.label }}</span>
              </RouterLink>
            </div>
          </template>
        </template>
      </nav>
      <div class="sidebar-footer">
        <b>Session keepalive</b>
        <span>5 分钟 · {{ keepalive.running ? '运行中' : '已停止' }} · {{ keepalive.lastSync }}</span>
      </div>
    </aside>

    <main class="main">
      <header class="topbar">
        <div class="title">
          <h1>{{ route.meta.title }}</h1>
          <p>GMV 控制台 · API v2</p>
        </div>
        <div class="top-actions">
          <div class="telemetry"><span class="dot" :class="{ paused: !keepalive.running }" />SESSION 保活</div>
          <div class="telemetry">interval <span class="code">5m</span></div>
          <div class="telemetry">last <span class="code">{{ keepalive.lastSync }}</span></div>
          <el-button type="primary" :loading="keepalive.refreshing" @click="refreshSession">立即保活</el-button>
          <el-dropdown trigger="click" popper-class="user-dropdown-popper" @command="handleUserCommand">
            <button class="user-menu-trigger" type="button">
              <span class="user-avatar">{{ userInitial }}</span>
              <span class="user-meta"><b>{{ displayName }}</b><small>{{ auth.session?.role }}</small></span>
            </button>
            <template #dropdown>
              <el-dropdown-menu>
                <el-dropdown-item command="profile">个人资料</el-dropdown-item>
                <el-dropdown-item divided command="logout">退出登录</el-dropdown-item>
              </el-dropdown-menu>
            </template>
          </el-dropdown>
        </div>
      </header>
      <RouterView />
    </main>

    <el-dialog v-model="profileDialogVisible" title="个人资料" width="460px">
      <el-form label-position="top" class="profile-form">
        <el-form-item label="用户名">
          <el-input :model-value="auth.session?.username" disabled />
        </el-form-item>
        <el-form-item label="当前角色">
          <el-input :model-value="auth.session?.role" disabled />
        </el-form-item>
        <el-form-item label="昵称">
          <el-input v-model="profileForm.nickname" placeholder="请输入显示昵称" />
        </el-form-item>
        <el-form-item label="新密码">
          <el-input v-model="profileForm.password" type="password" show-password placeholder="不修改请留空" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="profileDialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="savingProfile" @click="saveProfile">保存个人资料</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { useRoute, useRouter } from 'vue-router';
import { updateProfile } from '@/api/client';
import { menuRoutes } from '@/router';
import { useAuthStore } from '@/stores/auth';
import { useSessionKeepaliveStore } from '@/stores/sessionKeepalive';

const route = useRoute();
const router = useRouter();
const auth = useAuthStore();
const keepalive = useSessionKeepaliveStore();
const loggingOut = ref(false);
const profileDialogVisible = ref(false);
const savingProfile = ref(false);
const profileForm = reactive({ nickname: '', password: '' });
const displayName = computed(() => auth.session?.nickname?.trim() || auth.session?.username || '');
const userInitial = computed(() => displayName.value.slice(0, 1).toUpperCase() || 'U');
const groups = computed(() => [...new Set(menuRoutes.map((item) => item.group))]);
const grouped = computed(() =>
  menuRoutes.reduce(
    (acc, item) => {
      (acc[item.group] ||= []).push(item);
      return acc;
    },
    {} as Record<string, Array<(typeof menuRoutes)[number]>>,
  ),
);

onMounted(() => {
  keepalive.start();
});

onUnmounted(() => {
  keepalive.stop();
});

async function refreshSession() {
  try { await keepalive.refresh(); }
  catch (error) { ElMessage.error(error instanceof Error ? error.message : 'SESSION 保活失败'); }
}

function openProfile() {
  profileForm.nickname = auth.session?.nickname ?? '';
  profileForm.password = '';
  profileDialogVisible.value = true;
}

async function saveProfile() {
  savingProfile.value = true;
  try {
    const updated = await updateProfile({
      nickname: profileForm.nickname,
      password: profileForm.password || undefined,
    });
    auth.updateNickname(updated.nickname);
    profileForm.nickname = updated.nickname;
    profileForm.password = '';
    profileDialogVisible.value = false;
    ElMessage.success('个人资料已保存');
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '保存失败');
  } finally {
    savingProfile.value = false;
  }
}

async function handleUserCommand(command: string | number | object) {
  if (command === 'profile') {
    openProfile();
    return;
  }
  if (command === 'logout') await signOut();
}

async function signOut() {
  loggingOut.value = true;
  try {
    keepalive.stop();
    await auth.signOut();
    await router.replace('/login');
  } finally {
    loggingOut.value = false;
  }
}
</script>
