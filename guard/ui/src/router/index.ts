import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router';
import { ElMessage } from 'element-plus';
import AppShell from '@/components/AppShell.vue';
import { pinia } from '@/stores';
import { useAuthStore } from '@/stores/auth';
import { useExperimentalFeaturesStore } from '@/stores/experimentalFeatures';

export interface MenuRouteItem {
  path: string;
  label: string;
  icon: string;
  experimental?: boolean;
  children?: Array<Omit<MenuRouteItem, 'children'>>;
}

export const menuRoutes: MenuRouteItem[] = [
  { path: '/dashboard', label: 'Dashboard', icon: 'HomeFilled' },
  {
    path: '/gb28181/register',
    label: 'GB28181',
    icon: 'Platform',
    children: [
      { path: '/gb28181/register', label: '注册管理', icon: 'CircleCheck' },
      { path: '/gb28181/monitor', label: '监控信息', icon: 'Monitor' },
    ],
  },
  { path: '/onvif', label: 'ONVIF', icon: 'Connection', experimental: true },
  { path: '/streams', label: '流媒监控', icon: 'VideoCamera' },
  { path: '/ai', label: '智能分析', icon: 'DataAnalysis', experimental: true },
  { path: '/events', label: '告警与事件', icon: 'Bell', experimental: true },
  {
    path: '/integrations/apps',
    label: '三方集成',
    icon: 'Link',
    children: [
      { path: '/integrations/apps', label: '应用与凭证', icon: 'Key' },
      { path: '/integrations/http', label: 'HTTP 接入', icon: 'Document' },
      { path: '/integrations/mqtt', label: 'MQTT 接入', icon: 'Promotion' },
    ],
  },
  {
    path: '/system/health',
    label: '系统管理',
    icon: 'Setting',
    children: [
      { path: '/system/health', label: '系统健康', icon: 'Monitor' },
      { path: '/system/users', label: '用户管理', icon: 'User' },
    ],
  },
];

const routes: RouteRecordRaw[] = [
  { path: '/login', name: 'login', component: () => import('@/views/LoginView.vue'), meta: { title: '登录' } },
  {
    path: '/',
    component: AppShell,
    redirect: '/dashboard',
    meta: { requiresAuth: true },
    children: [
      { path: 'dashboard', component: () => import('@/views/DashboardView.vue'), meta: { title: 'Dashboard' } },
      { path: 'gb28181', redirect: '/gb28181/register' },
      { path: 'gb28181/register', component: () => import('@/views/Gb28181View.vue'), meta: { title: '注册管理' } },
      { path: 'gb28181/monitor', component: () => import('@/views/Gb28181MonitorView.vue'), meta: { title: '监控信息' } },
      { path: 'onvif', component: () => import('@/views/OnvifView.vue'), meta: { title: 'ONVIF', experimental: true } },
      { path: 'streams', component: () => import('@/views/StreamsView.vue'), meta: { title: '流媒监控' } },
      { path: 'ai', component: () => import('@/views/AiView.vue'), meta: { title: '智能分析', experimental: true } },
      { path: 'nodes', redirect: '/system/health' },
      { path: 'events', component: () => import('@/views/EventsView.vue'), meta: { title: '告警与事件', experimental: true } },
      { path: 'integrations', redirect: '/integrations/apps' },
      { path: 'integrations/apps', component: () => import('@/views/IntegrationsView.vue'), meta: { title: '应用与凭证' } },
      { path: 'integrations/http', component: () => import('@/views/HttpIntegrationView.vue'), meta: { title: 'HTTP 接入' } },
      { path: 'integrations/mqtt', component: () => import('@/views/MqttIntegrationView.vue'), meta: { title: 'MQTT 接入' } },
      { path: 'system', redirect: '/system/health' },
      { path: 'system/health', component: () => import('@/views/SystemHealthView.vue'), meta: { title: '系统健康' } },
      { path: 'system/users', component: () => import('@/views/UserManagementView.vue'), meta: { title: '用户管理' } },
    ],
  },
  { path: '/:pathMatch(.*)*', redirect: '/dashboard' },
];

const router = createRouter({ history: createWebHistory(), routes });

router.beforeEach(async (to) => {
  const auth = useAuthStore(pinia);
  const experimental = useExperimentalFeaturesStore(pinia);

  if (to.name === 'login') {
    if (!auth.session) return true;
    return typeof to.query.redirect === 'string' ? to.query.redirect : '/dashboard';
  }
  const authenticated = auth.session ? true : await auth.restore();
  if (to.matched.some((record) => record.meta.requiresAuth) && !authenticated) {
    return { name: 'login', query: { redirect: to.fullPath } };
  }
  experimental.sync(auth.session?.username, auth.isAdmin);
  if (to.matched.some((record) => record.meta.experimental) && !experimental.enabled) {
    ElMessage.warning('该功能当前处于实验隐藏状态');
    return '/dashboard';
  }
  return true;
});

export default router;
