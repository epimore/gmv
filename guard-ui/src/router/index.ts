import { createRouter, createWebHistory, type RouteRecordRaw } from 'vue-router';
import AppShell from '@/components/AppShell.vue';
import { pinia } from '@/stores';
import { useAuthStore } from '@/stores/auth';

export const menuRoutes = [
  { path: '/dashboard', label: '总览', icon: '总', group: '控制台' },
  { path: '/nodes', label: '节点', icon: '节', group: '资源' },
  { path: '/devices', label: '设备', icon: '设', group: '资源' },
  { path: '/streams', label: '流媒体', icon: '流', group: '业务' },
  { path: '/ai', label: '智能分析', icon: '智', group: '业务' },
  { path: '/allocations', label: '调度与租约', icon: '调', group: '治理' },
  { path: '/events', label: '事件中心', icon: '事', group: '治理' },
  { path: '/integrations', label: '集成', icon: '集', group: '外部' },
  { path: '/system', label: '系统', icon: '系', group: '外部' },
] as const;

const routes: RouteRecordRaw[] = [
  { path: '/login', name: 'login', component: () => import('@/views/LoginView.vue'), meta: { title: '登录' } },
  {
    path: '/',
    component: AppShell,
    redirect: '/dashboard',
    meta: { requiresAuth: true },
    children: [
      { path: 'dashboard', component: () => import('@/views/DashboardView.vue'), meta: { title: '总览' } },
      { path: 'nodes', component: () => import('@/views/NodesView.vue'), meta: { title: '节点' } },
      { path: 'devices', component: () => import('@/views/DevicesView.vue'), meta: { title: '设备' } },
      { path: 'streams', component: () => import('@/views/StreamsView.vue'), meta: { title: '流媒体' } },
      { path: 'ai', component: () => import('@/views/AiView.vue'), meta: { title: '智能分析' } },
      { path: 'allocations', component: () => import('@/views/AllocationsView.vue'), meta: { title: '调度与租约' } },
      { path: 'events', component: () => import('@/views/EventsView.vue'), meta: { title: '事件中心' } },
      { path: 'integrations', component: () => import('@/views/IntegrationsView.vue'), meta: { title: '集成' } },
      { path: 'system', component: () => import('@/views/SystemView.vue'), meta: { title: '系统' } },
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
