import { currentSession, errorMessage } from '@/api/client';
import { useAuthStore } from '@/stores/auth';
import { defineStore } from 'pinia';

const DEFAULT_KEEPALIVE_INTERVAL_MS = 5 * 60 * 1000;

let timer: number | undefined;
let inFlight = false;

export const useSessionKeepaliveStore = defineStore('sessionKeepalive', {
  state: () => ({
    enabled: true,
    running: false,
    refreshing: false,
    intervalMs: DEFAULT_KEEPALIVE_INTERVAL_MS,
    lastSync: '-',
    lastError: '',
  }),
  getters: {
    intervalMinutes: (state) => Math.round(state.intervalMs / 60_000),
    statusLabel: (state) => {
      if (!state.enabled) return '已关闭';
      if (state.refreshing) return '保活中';
      if (state.lastError) return '异常';
      return state.running ? '运行中' : '未启动';
    },
  },
  actions: {
    start() {
      if (!this.enabled) return;
      if (timer !== undefined) return;
      this.running = true;
      void this.refresh().catch(() => undefined);
      timer = window.setInterval(() => {
        void this.refresh().catch(() => undefined);
      }, this.intervalMs);
    },
    stop() {
      if (timer !== undefined) {
        window.clearInterval(timer);
        timer = undefined;
      }
      this.running = false;
    },
    configure(enabled: boolean, intervalMinutes: number) {
      const normalizedMinutes = Math.max(1, Math.min(60, Math.round(intervalMinutes)));
      this.enabled = enabled;
      this.intervalMs = normalizedMinutes * 60_000;
      this.stop();
      if (enabled) this.start();
    },
    async refresh() {
      if (inFlight) return;
      inFlight = true;
      this.refreshing = true;
      try {
        const session = await currentSession();
        useAuthStore().updateSession(session);
        this.lastSync = new Date().toLocaleTimeString('zh-CN', { hour12: false });
        this.lastError = '';
      } catch (error) {
        this.lastError = errorMessage(error, '会话保活失败');
        throw error;
      } finally {
        this.refreshing = false;
        inFlight = false;
      }
    },
  },
});
