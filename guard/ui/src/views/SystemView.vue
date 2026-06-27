<template>
  <div class="page-grid">
    <GlassPanel class="span-5" title="系统健康" subtitle="TLS 默认、NTP/chrony、存储后端">
      <div class="kv">
        <div class="kv-item"><span>展示品牌</span><b>GMV</b></div>
        <div class="kv-item"><span>后端服务</span><b>guard</b></div>
        <div class="kv-item"><span>存储/Outbox</span><b>{{ readyStatus }}</b></div>
        <div class="kv-item"><span>TLS</span><b>{{ tlsStatus }}</b></div>
        <div class="kv-item"><span>进程存活</span><b>{{ liveStatus }}</b></div>
        <div class="kv-item"><span>API</span><b>v2</b></div>
      </div>
    </GlassPanel>

    <GlassPanel class="span-7" title="系统任务" subtitle="备份、恢复、迁移、证书和审计">
      <el-table :data="systemJobs" height="300">
        <el-table-column prop="job_type" label="任务" width="150" />
        <el-table-column label="状态" width="120">
          <template #default="{ row }"><StatusPill :label="row.status.toUpperCase()" :tone="row.status === 'failed' ? 'danger' : row.status" /></template>
        </el-table-column>
        <el-table-column label="进度" width="180"><template #default="{ row }"><el-progress :percentage="row.progress_percent" /></template></el-table-column>
        <el-table-column label="说明"><template #default="{ row }">{{ row.error || row.message || '-' }}</template></el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-5" title="个人资料" subtitle="维护自己的昵称与密码">
      <el-form label-position="top" class="profile-form">
        <el-form-item label="用户名">
          <el-input :model-value="session?.username" disabled />
        </el-form-item>
        <el-form-item label="当前角色">
          <el-input :model-value="session ? roleLabel(session.role) : ''" disabled />
        </el-form-item>
        <el-form-item label="昵称">
          <el-input v-model="profileForm.nickname" placeholder="请输入显示昵称" />
        </el-form-item>
        <el-form-item label="新密码">
          <el-input v-model="profileForm.password" type="password" show-password placeholder="不修改请留空" />
        </el-form-item>
        <div class="toolbar">
          <el-button type="primary" :loading="savingProfile" @click="saveProfile">保存个人资料</el-button>
          <span class="code">guard_user</span>
        </div>
      </el-form>
    </GlassPanel>

    <GlassPanel class="span-7" title="用户管理" subtitle="admin 创建用户、配置角色、重置密码">
      <div class="toolbar">
        <el-button type="primary" :disabled="!canManageUsers" @click="openCreateUser">创建用户</el-button>
        <el-button :loading="loadingUsers" @click="loadSecurityState">刷新</el-button>
        <span class="code">角色：viewer / operator / admin</span>
      </div>
      <el-alert v-if="!canManageUsers" title="当前用户不是 admin，只能维护自己的基本信息。" type="warning" :closable="false" show-icon />
      <el-table :data="users" height="300" style="margin-top: 12px;">
        <el-table-column prop="username" label="用户名" min-width="130" />
        <el-table-column prop="nickname" label="昵称" min-width="140" />
        <el-table-column label="角色" width="120">
          <template #default="{ row }"><StatusPill :label="roleLabel(row.role)" :tone="row.role === 'admin' ? 'danger' : row.role === 'operator' ? 'info' : 'ready'" /></template>
        </el-table-column>
        <el-table-column label="状态" width="100">
          <template #default="{ row }"><StatusPill :label="row.enabled ? '启用' : '停用'" :tone="row.enabled ? 'ready' : 'danger'" /></template>
        </el-table-column>
        <el-table-column label="操作" width="210" fixed="right">
          <template #default="{ row }">
            <el-button size="small" :disabled="!canManageUsers" @click="openEditUser(row)">编辑</el-button>
            <el-button size="small" type="warning" :disabled="!canManageUsers" @click="resetUserPassword(row)">重置密码</el-button>
          </template>
        </el-table-column>
      </el-table>
    </GlassPanel>

    <GlassPanel class="span-12" title="安全操作" subtitle="危险动作需二次确认与审计">
      <div class="toolbar">
        <el-button :disabled="!canManageUsers" :loading="runningJob" @click="runJob('backup')">开始备份任务</el-button>
        <el-button :disabled="!canManageUsers" :loading="runningJob" @click="runJob('migrate')">迁移检查</el-button>
        <el-button :disabled="!canManageUsers" :loading="runningJob" @click="runJob('reconcile')">执行对账</el-button>
        <el-button type="warning" :disabled="!canManageUsers" :loading="runningJob" @click="runJob('restore')">创建恢复任务</el-button>
      </div>
      <OrbitChart :option="jobChart" />
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
          <el-input v-model="userForm.password" type="password" show-password :placeholder="editingUser ? '不重置请留空' : '请输入初始密码'" />
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
import OrbitChart from '@/components/OrbitChart.vue';
import StatusPill from '@/components/StatusPill.vue';
import { lineOption } from '@/data/charts';
import {
  createUser,
  currentProfile,
  healthLive,
  healthReady,
  currentSession,
  listSystemJobs,
  listUsers,
  startSystemJob,
  updateProfile,
  updateUser,
  type SystemJobInfo,
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
const session = computed(() => auth.session);
const profileForm = reactive({ nickname: '', password: '' });
const users = ref<UserInfo[]>([]);
const systemJobs = ref<SystemJobInfo[]>([]);
const liveStatus = ref('检查中');
const readyStatus = ref('检查中');
const tlsStatus = window.location.protocol === 'https:' ? '启用' : '本地 HTTP';
const loadingUsers = ref(false);
const savingProfile = ref(false);
const savingUser = ref(false);
const runningJob = ref(false);
const userDialogVisible = ref(false);
const editingUser = ref<UserInfo | null>(null);
const userForm = reactive<UserForm>({ username: '', nickname: '', role: 'viewer', password: '', enabled: true });
const canManageUsers = computed(() => auth.isAdmin);
const jobChart = computed(() => lineOption('任务进度', systemJobs.value.map((job) => job.progress_percent), systemJobs.value.map((job) => job.job_type), '#a875ff'));

function roleLabel(role: Role) {
  return { viewer: 'viewer 只读', operator: 'operator 操作', admin: 'admin 管理' }[role];
}

async function loadSystemState() {
  try {
    const [live, ready, jobs] = await Promise.all([healthLive(), healthReady(), listSystemJobs()]);
    liveStatus.value = live.status;
    readyStatus.value = ready.status;
    systemJobs.value = jobs.slice().reverse();
  } catch (error) {
    liveStatus.value = '异常';
    readyStatus.value = '异常';
    ElMessage.error(error instanceof Error ? error.message : '系统状态加载失败');
  }
}

async function runJob(jobType: SystemJobInfo['job_type']) {
  runningJob.value = true;
  try {
    await startSystemJob(jobType);
    ElMessage.success('系统任务已创建');
    await loadSystemState();
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '任务创建失败');
  } finally {
    runningJob.value = false;
  }
}

async function loadSecurityState() {
  loadingUsers.value = true;
  try {
    const current = await currentSession();
    auth.updateSession(current);
    profileForm.nickname = current.nickname || current.username;
    users.value = current.role === 'admin' ? await listUsers() : [await currentProfile()];
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '加载用户信息失败');
  } finally {
    loadingUsers.value = false;
  }
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
    await loadSecurityState();
    ElMessage.success('个人资料已保存');
  } catch (error) {
    ElMessage.error(error instanceof Error ? error.message : '保存失败');
  } finally {
    savingProfile.value = false;
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

async function resetUserPassword(user: UserInfo) {
  openEditUser(user);
  await ElMessageBox.alert('请在弹窗中输入新密码并保存。', '重置用户密码');
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
    ElMessage.error(error instanceof Error ? error.message : '保存用户失败');
  } finally {
    savingUser.value = false;
  }
}

onMounted(() => { void Promise.all([loadSecurityState(), loadSystemState()]); });
</script>
