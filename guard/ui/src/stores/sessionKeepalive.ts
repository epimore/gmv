import { currentSession } from '@/api/client';
import { useAuthStore } from '@/stores/auth';
import { defineStore } from 'pinia';

const KEEPALIVE_INTERVAL_MS = 5 * 60 * 1000;

let timer: number | undefined;
let inFlight = false;

export const useSessionKeepaliveStore = defineStore('sessionKeepalive', {
  state: () => ({ running: false, refreshing: false, intervalMs: KEEPALIVE_INTERVAL_MS, lastSync: '-' }),
  actions: {
    start() {
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
    async refresh() {
      if (inFlight) return;
      inFlight = true;
      this.refreshing = true;
      try {
        const session = await currentSession();
        useAuthStore().updateSession(session);
        this.lastSync = new Date().toLocaleTimeString('zh-CN', { hour12: false });
      } finally {
        this.refreshing = false;
        inFlight = false;
      }
    },
  },
});
