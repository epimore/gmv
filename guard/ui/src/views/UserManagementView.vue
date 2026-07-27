<template>
  <div class="page-grid">
    <GlassPanel class="span-12" title="用户管理" subtitle="admin 创建用户、配置角色、有效期和重置密码">
      <div class="toolbar">
        <el-button type="primary" :disabled="!canManageUsers" @click="openCreateUser">创建用户</el-button>
        <el-button :loading="loadingUsers" @click="loadSecurityState">刷新</el-button>
        <span class="code">角色：viewer / operator / admin</span>
      </div>
      <el-alert v-if="!canManageUsers" title="当前用户不是 admin，只能维护自己的基本信息。" type="warning" :closable="false" show-icon />
      <el-table :data="users" height="300" style="margin-top: 12px;">
        <el-table-column prop="username" label="用户名" min-width="130" />
        <el-table-column prop="nickname" label="昵称" min-width="140" />
        <el-table-column label="角色" width="220">
          <template #default="{ row }">
            <StatusPill :label="roleLabel(row.role)"
              :tone="row.role === 'admin' ? 'danger' : row.role === 'operator' ? 'info' : 'ready'" />
          </template>
        </el-table-column>
        <el-table-column label="状态" width="100">
          <template #default="{ row }">
            <StatusPill :label="userStatusLabel(row)" :tone="row.enabled && !userExpired(row) ? 'ready' : 'danger'" />
          </template>
        </el-table-column>
        <el-table-column label="有效期" width="190">
          <template #default="{ row }">{{ formatUserExpiration(row.expires_at_ms) }}</template>
        </el-table-column>
        <el-table-column label="操作" width="210" fixed="right">
          <template #default="{ row }">
            <el-button size="small" :disabled="!canManageUsers" @click="openEditUser(row)">编辑</el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <el-dialog v-model="userDialogVisible" :title="editingUser ? '编辑用户' : '创建用户'" width="520px">
      <el-form label-position="top">
        <el-form-item label="用户名">
          <el-input v-model="userForm.username" :disabled="Boolean(editingUser)" placeholder="例如 ops" />
        </el-form-item>
        <el-form-item label="昵称">
          <el-input v-model="userForm.nickname" placeholder="显示昵称" />
        </el-form-item>
        <el-form-item label="角色">
          <el-select v-model="userForm.role" style="width: 100%;">
            <el-option label="viewer · 媒体查看与控制" value="viewer" />
            <el-option label="operator · 业务操作" value="operator" />
            <el-option label="admin · 系统管理" value="admin" />
          </el-select>
        </el-form-item>
        <el-form-item :label="editingUser ? '重置密码' : '初始密码'">
          <el-input v-model="userForm.password" type="password" show-password
            :placeholder="editingUser ? '不重置请留空' : '请输入初始密码'" />
        </el-form-item>
        <el-form-item label="状态">
          <el-switch v-model="userForm.enabled" active-text="启用" inactive-text="停用" />
        </el-form-item>
        <el-form-item label="有效期">
          <el-select v-model="userForm.expirationPreset" style="width: 100%;">
            <el-option label="1小时" value="hour" />
            <el-option label="1天" value="day" />
            <el-option label="1周" value="week" />
            <el-option label="1月" value="month" />
            <el-option label="永久" value="permanent" />
            <el-option label="自定义" value="custom" />
          </el-select>
        </el-form-item>
        <el-form-item v-if="userForm.expirationPreset === 'custom'" label="自定义到期时间">
          <el-date-picker v-model="userForm.customExpiresAt" type="datetime" format="YYYY-MM-DD HH:mm:ss"
            placeholder="请选择到期时间" :clearable="true" :disabled-date="disablePastDate" style="width: 100%;" />
        </el-form-item>
      </el-form>
      <template #footer>
        <el-button @click="userDialogVisible = false">取消</el-button>
        <el-button type="primary" :loading="savingUser" @click="saveUser">保存</el-button>
      </template>
    </el-dialog>
  </div>
</template>

<script setup lang="ts">
import { computed, onMounted, reactive, ref } from 'vue';
import { ElMessage } from 'element-plus';
import GlassPanel from '@/components/GlassPanel.vue';
import StatusPill from '@/components/StatusPill.vue';
import {
  createUser,
  currentProfile,
  currentSession,
  errorMessage,
  listUsers,
  updateUser,
  type UserInfo,
} from '@/api/client';
import { useAuthStore } from '@/stores/auth';

type Role = UserInfo['role'];
type ExpirationPreset = 'hour' | 'day' | 'week' | 'month' | 'permanent' | 'custom';

type UserForm = {
  username: string;
  nickname: string;
  role: Role;
  password: string;
  enabled: boolean;
  expirationPreset: ExpirationPreset;
  customExpiresAt?: Date;
};

const auth = useAuthStore();
const users = ref<UserInfo[]>([]);
const loadingUsers = ref(false);
const savingUser = ref(false);
const userDialogVisible = ref(false);
const editingUser = ref<UserInfo | null>(null);
const userForm = reactive<UserForm>({
  username: '', nickname: '', role: 'viewer', password: '', enabled: true,
  expirationPreset: 'permanent', customExpiresAt: undefined,
});
const canManageUsers = computed(() => auth.isAdmin);

function roleLabel(role: Role) {
  return { viewer: 'viewer 媒体控制', operator: 'operator 操作', admin: 'admin 管理' }[role];
}

function userExpired(user: UserInfo) {
  return user.expires_at_ms !== null && user.expires_at_ms <= Date.now();
}

function userStatusLabel(user: UserInfo) {
  if (!user.enabled) return '停用';
  return userExpired(user) ? '已过期' : '启用';
}

function formatUserExpiration(expiresAtMs: number | null) {
  if (expiresAtMs === null) return '永久';
  return new Date(expiresAtMs).toLocaleString('zh-CN', { hour12: false });
}

function disablePastDate(date: Date) {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  return date.getTime() < today.getTime();
}

async function loadSecurityState() {
  loadingUsers.value = true;
  try {
    const current = await currentSession();
    auth.updateSession(current);
    users.value = current.role === 'admin' ? await listUsers() : [await currentProfile()];
  } catch (error) {
    ElMessage.error(errorMessage(error, '加载用户信息失败'));
  } finally {
    loadingUsers.value = false;
  }
}

function openCreateUser() {
  editingUser.value = null;
  Object.assign(userForm, {
    username: '', nickname: '', role: 'viewer', password: '', enabled: true,
    expirationPreset: 'permanent', customExpiresAt: undefined,
  });
  userDialogVisible.value = true;
}

function openEditUser(user: UserInfo) {
  editingUser.value = user;
  Object.assign(userForm, {
    username: user.username,
    nickname: user.nickname,
    role: user.role,
    password: '',
    enabled: user.enabled,
    expirationPreset: user.expires_at_ms === null ? 'permanent' : 'custom',
    customExpiresAt: user.expires_at_ms === null ? undefined : new Date(user.expires_at_ms),
  });
  userDialogVisible.value = true;
}

async function saveUser() {
  if (!userForm.username.trim()) {
    ElMessage.warning('请输入用户名');
    return;
  }
  if (!editingUser.value && !userForm.password) {
    ElMessage.warning('创建用户需要初始密码');
    return;
  }
  const now = new Date();
  let expiresAtMs: number | null = null;
  if (userForm.expirationPreset === 'custom') {
    expiresAtMs = userForm.customExpiresAt?.getTime() ?? 0;
  } else if (userForm.expirationPreset !== 'permanent') {
    const expiresAt = new Date(now);
    if (userForm.expirationPreset === 'hour') expiresAt.setHours(expiresAt.getHours() + 1);
    else if (userForm.expirationPreset === 'day') expiresAt.setDate(expiresAt.getDate() + 1);
    else if (userForm.expirationPreset === 'week') expiresAt.setDate(expiresAt.getDate() + 7);
    else {
      const day = expiresAt.getDate();
      expiresAt.setDate(1);
      expiresAt.setMonth(expiresAt.getMonth() + 1);
      const lastDay = new Date(expiresAt.getFullYear(), expiresAt.getMonth() + 1, 0).getDate();
      expiresAt.setDate(Math.min(day, lastDay));
    }
    expiresAtMs = expiresAt.getTime();
  }
  if (expiresAtMs !== null && expiresAtMs <= Date.now()) {
    ElMessage.warning('请选择晚于当前时间的有效期');
    return;
  }
  savingUser.value = true;
  try {
    if (editingUser.value) {
      await updateUser(userForm.username, {
        role: userForm.role,
        nickname: userForm.nickname,
        password: userForm.password || null,
        enabled: userForm.enabled,
        expires_at_ms: expiresAtMs,
      });
    } else {
      await createUser({
        username: userForm.username,
        nickname: userForm.nickname,
        role: userForm.role,
        password: userForm.password,
        enabled: userForm.enabled,
        expires_at_ms: expiresAtMs,
      });
    }
    await loadSecurityState();
    userDialogVisible.value = false;
    userForm.password = '';
    ElMessage.success('用户信息已保存');
  } catch (error) {
    ElMessage.error(errorMessage(error, '保存用户失败'));
  } finally {
    savingUser.value = false;
  }
}

onMounted(() => { void loadSecurityState(); });
</script>
