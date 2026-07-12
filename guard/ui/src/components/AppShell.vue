<template>
  <div class="app-shell" :class="{ 'is-sidebar-collapsed': menuCollapsed }">
    <div class="console-texture" />
    <aside class="sidebar">
      <RouterLink class="brand" to="/dashboard" aria-label="GMV 总览">
        <img class="brand-mark" src="/favicon.ico" alt="" />
        <div class="brand-copy">
          <div class="brand-title">GMV</div>
          <!-- <div class="brand-sub">kang9095@126.com</div> -->
        </div>
      </RouterLink>
      <el-menu class="sidebar-menu" :collapse="menuCollapsed" :default-active="route.path" :default-openeds="openMenus"
        router>
        <template v-for="item in menuRoutes" :key="item.path">
          <el-sub-menu v-if="item.children?.length" :index="item.path">
            <template #title>
              <el-icon>
                <component :is="menuIcon(item.icon)" />
              </el-icon>
              <span>{{ item.label }}</span>
            </template>
            <el-menu-item v-for="child in item.children" :key="child.path" :index="child.path">
              <el-icon>
                <component :is="menuIcon(child.icon)" />
              </el-icon>
              <span>{{ child.label }}</span>
            </el-menu-item>
          </el-sub-menu>
          <el-menu-item v-else :index="item.path">
            <el-icon>
              <component :is="menuIcon(item.icon)" />
            </el-icon>
            <span>{{ item.label }}</span>
          </el-menu-item>
        </template>
      </el-menu>
    </aside>

    <main class="main">
      <header class="main-header">
        <div class="main-header-left">
          <el-button class="layout-collapse" :icon="menuCollapsed ? Expand : Fold" text
            @click="menuCollapsed = !menuCollapsed" />
          <span class="welcome-text">欢迎进入 GMV 音视频AI监控平台</span>
        </div>
        <el-dropdown trigger="click" popper-class="user-dropdown-popper" @command="handleUserCommand">
          <button class="user-menu-trigger" type="button">
            <span class="user-avatar">{{ userInitial }}</span>
            <span class="user-meta"><b>{{ displayName }}</b><small>{{ auth.session?.role }}</small></span>
          </button>
          <template #dropdown>
            <el-dropdown-menu>
              <el-dropdown-item command="profile">个人资料</el-dropdown-item>
              <el-dropdown-item command="keepalive">会话保活</el-dropdown-item>
              <el-dropdown-item divided command="logout">退出登录</el-dropdown-item>
            </el-dropdown-menu>
          </template>
        </el-dropdown>
      </header>
      <header class="topbar">
        <div class="title">
          <h1>{{ route.meta.title }}</h1>
          <p>GMV 控制台 · API v2</p>
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

    <el-dialog v-model="keepaliveDialogVisible" title="会话保活" width="480px">
      <div class="keepalive-summary">
        <div class="kv-item"><span>保活状态</span><b>{{ keepalive.statusLabel }}</b></div>
        <div class="kv-item"><span>是否启用</span><b>{{ keepalive.enabled ? '启用' : '关闭' }}</b></div>
        <div class="kv-item"><span>心跳周期</span><b>{{ keepalive.intervalMinutes }} 分钟</b></div>
        <div class="kv-item"><span>上次保活</span><b class="code">{{ keepalive.lastSync }}</b></div>
      </div>
      <el-form label-position="top" class="profile-form keepalive-form">
        <el-form-item label="启用会话保活">
          <el-switch v-model="keepaliveForm.enabled" active-text="启用" inactive-text="关闭" />
        </el-form-item>
        <el-form-item label="保活心跳周期（分钟，仅当前会话）">
          <el-input-number v-model="keepaliveForm.intervalMinutes" :min="1" :max="60" :step="1"
            controls-position="right" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="keepaliveDialogVisible = false">取消</el-button>
        <el-button type="primary" @click="saveKeepaliveSettings">保存设置</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import { Bell, CircleCheck, Connection, Cpu, DataAnalysis, Expand, Fold, HomeFilled, Link, Menu as MenuIcon, Monitor, Platform, Setting, User, VideoCamera } from '@element-plus/icons-vue';
import { useRoute, useRouter } from 'vue-router';
import { errorMessage, updateProfile } from '@/api/client';
import { menuRoutes } from '@/router';
import { useAuthStore } from '@/stores/auth';
import { useSessionKeepaliveStore } from '@/stores/sessionKeepalive';

const route = useRoute();
const router = useRouter();
const auth = useAuthStore();
const keepalive = useSessionKeepaliveStore();
const loggingOut = ref(false);
const profileDialogVisible = ref(false);
const keepaliveDialogVisible = ref(false);
const savingProfile = ref(false);
const profileForm = reactive({ nickname: '', password: '' });
const keepaliveForm = reactive({ enabled: true, intervalMinutes: 5 });
const menuCollapsed = ref(false);
const displayName = computed(() => auth.session?.nickname?.trim() || auth.session?.username || '');
const userInitial = computed(() => displayName.value.slice(0, 1).toUpperCase() || 'U');
const openMenus = computed(() => menuRoutes.filter((item) => item.children?.some((child) => route.path.startsWith(child.path))).map((item) => item.path));
const menuIcons = { Bell, CircleCheck, Connection, Cpu, DataAnalysis, HomeFilled, Link, Menu: MenuIcon, Monitor, Platform, Setting, User, VideoCamera };

function menuIcon(name: string) {
  return menuIcons[name as keyof typeof menuIcons] ?? MenuIcon;
}

onMounted(() => {
  keepalive.start();
});

onUnmounted(() => {
  keepalive.stop();
});

function openProfile() {
  profileForm.nickname = auth.session?.nickname ?? '';
  profileForm.password = '';
  profileDialogVisible.value = true;
}

function openKeepaliveSettings() {
  keepaliveForm.enabled = keepalive.enabled;
  keepaliveForm.intervalMinutes = keepalive.intervalMinutes;
  keepaliveDialogVisible.value = true;
}

function saveKeepaliveSettings() {
  keepalive.configure(keepaliveForm.enabled, keepaliveForm.intervalMinutes);
  keepaliveForm.intervalMinutes = keepalive.intervalMinutes;
  keepaliveDialogVisible.value = false;
  ElMessage.success('会话保活设置已更新');
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
    ElMessage.error(errorMessage(error, '保存失败'));
  } finally {
    savingProfile.value = false;
  }
}

async function handleUserCommand(command: string | number | object) {
  if (command === 'profile') {
    openProfile();
    return;
  }
  if (command === 'keepalive') {
    openKeepaliveSettings();
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
