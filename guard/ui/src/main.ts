import { createApp } from 'vue';
import ElementPlus from 'element-plus';
import 'element-plus/dist/index.css';
import './styles/main.css';
import App from './App.vue';
import router from './router';
import { setUnauthorizedHandler } from '@/api/client';
import { pinia } from '@/stores';
import { useAuthStore } from '@/stores/auth';

setUnauthorizedHandler(() => {
  useAuthStore(pinia).clearSession();
  if (router.currentRoute.value.name !== 'login') {
    void router.replace({ name: 'login', query: { redirect: router.currentRoute.value.fullPath } });
  }
});

createApp(App).use(pinia).use(router).use(ElementPlus).mount('#app');
