import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router';
import AppShell from '@/components/AppShell.vue';
import { pinia } from '@/stores';
import { useAuthStore } from '@/stores/auth';

export interface MenuRouteItem {
  path: string;
  label: string;
  icon: string;
  children?: Array<Omit<MenuRouteItem, 'children'>>;
}

export const menuRoutes: MenuRouteItem[] = [
  { path: '/dashboard', label: '总览', icon: 'HomeFilled' },
  {
    path: '/gb28181/register',
    label: 'GB28181',
    icon: 'Platform',
    children: [
      { path: '/gb28181/register', label: '注册管理', icon: 'CircleCheck' },
      { path: '/gb28181/monitor', label: '监控信息', icon: 'Monitor' },
    ],
  },
  { path: '/onvif', label: 'ONVIF', icon: 'Connection' },
  { path: '/streams', label: '流媒体', icon: 'VideoCamera' },
  { path: '/ai', label: '智能分析', icon: 'DataAnalysis' },
  { path: '/nodes', label: '节点', icon: 'Cpu' },
  { path: '/events', label: '事件中心', icon: 'Bell' },
  { path: '/integrations', label: '集成', icon: 'Link' },
  {
    path: '/system/health',
    label: '系统',
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
      { path: 'dashboard', component: () => import('@/views/DashboardView.vue'), meta: { title: '总览' } },
      { path: 'devices', component: () => import('@/views/DevicesView.vue'), meta: { title: '设备' } },
      { path: 'gb28181', redirect: '/gb28181/register' },
      { path: 'gb28181/register', component: () => import('@/views/Gb28181View.vue'), meta: { title: '注册管理' } },
      { path: 'gb28181/monitor', component: () => import('@/views/Gb28181MonitorView.vue'), meta: { title: '监控信息' } },
      { path: 'onvif', component: () => import('@/views/OnvifView.vue'), meta: { title: 'ONVIF' } },
      { path: 'streams', component: () => import('@/views/StreamsView.vue'), meta: { title: '流媒体' } },
      { path: 'ai', component: () => import('@/views/AiView.vue'), meta: { title: '智能分析' } },
      { path: 'nodes', component: () => import('@/views/NodesView.vue'), meta: { title: '节点' } },
      { path: 'events', component: () => import('@/views/EventsView.vue'), meta: { title: '事件中心' } },
      { path: 'integrations', component: () => import('@/views/IntegrationsView.vue'), meta: { title: '集成' } },
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
  const authenticated = auth.session ? true : await auth.restore();

  if (to.name === 'login') {
    if (!authenticated) return true;
    return typeof to.query.redirect === 'string' ? to.query.redirect : '/dashboard';
  }
  if (to.matched.some((record) => record.meta.requiresAuth) && !authenticated) {
    return { name: 'login', query: { redirect: to.fullPath } };
  }
  return true;
});

export default router;
