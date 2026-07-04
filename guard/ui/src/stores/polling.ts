import { pollEvents } from '@/api/client';
import { defineStore } from 'pinia';

let timer: number | undefined;
let inFlight = false;

export const usePollingStore = defineStore('polling', {
  state: () => ({ paused: false, afterId: '', nextCursor: '', intervalMs: 5000, lastSync: '-' }),
  actions: {
    toggle() { this.paused = !this.paused; },
    start() {
      if (timer !== undefined) return;
      void this.advance().catch(() => undefined);
      timer = window.setInterval(() => {
        void this.advance().catch(() => undefined);
      }, this.intervalMs);
    },
    stop() {
      if (timer === undefined) return;
      window.clearInterval(timer);
      timer = undefined;
    },
    async advance() {
      if (this.paused || inFlight) return;
      inFlight = true;
      try {
        const page = await pollEvents(this.afterId || undefined);
        if (page.next_after_id) { this.afterId = page.next_after_id; this.nextCursor = page.next_after_id; }
        this.lastSync = new Date().toLocaleTimeString('zh-CN', { hour12: false });
      } finally {
        inFlight = false;
      }
    },
  },
});
