<template>
  <div class="page-grid">
    <GlassPanel class="span-12" title="用户管理" subtitle="admin 创建用户、配置角色、重置密码">
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
            <StatusPill :label="row.enabled ? '启用' : '停用'" :tone="row.enabled ? 'ready' : 'danger'" />
          </template>
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
            <el-option label="viewer · 只读观测" value="viewer" />
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
import { ElMessage, ElMessageBox } from 'element-plus';
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

type UserForm = {
  username: string;
  nickname: string;
  role: Role;
  password: string;
  enabled: boolean;
};

const auth = useAuthStore();
const users = ref<UserInfo[]>([]);
const loadingUsers = ref(false);
const savingUser = ref(false);
const userDialogVisible = ref(false);
const editingUser = ref<UserInfo | null>(null);
const userForm = reactive<UserForm>({ username: '', nickname: '', role: 'viewer', password: '', enabled: true });
const canManageUsers = computed(() => auth.isAdmin);

function roleLabel(role: Role) {
  return { viewer: 'viewer 只读', operator: 'operator 操作', admin: 'admin 管理' }[role];
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
  Object.assign(userForm, { username: '', nickname: '', role: 'viewer', password: '', enabled: true });
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
  savingUser.value = true;
  try {
    if (editingUser.value) {
      await updateUser(userForm.username, {
        role: userForm.role,
        nickname: userForm.nickname,
        password: userForm.password || null,
        enabled: userForm.enabled,
      });
    } else {
      await createUser({ ...userForm });
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
