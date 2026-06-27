import { pollEvents } from '@/api/client';
import { defineStore } from 'pinia';

export const usePollingStore = defineStore('polling', {
  state: () => ({ paused: false, afterId: '', nextCursor: '', intervalMs: 3000, lastSync: '-' }),
  actions: {
    toggle() { this.paused = !this.paused; },
    async advance() {
      if (this.paused) return;
      const page = await pollEvents(this.afterId || undefined);
      if (page.next_after_id) { this.afterId = page.next_after_id; this.nextCursor = page.next_after_id; }
      this.lastSync = new Date().toLocaleTimeString('zh-CN', { hour12: false });
    },
  },
});
