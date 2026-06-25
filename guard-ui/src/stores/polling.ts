import { liveApi, pollEvents } from '@/api/client';
import { defineStore } from 'pinia';

export const usePollingStore = defineStore('polling', {
  state: () => ({
    paused: false,
    afterId: 'evt_000245',
    nextCursor: 'cur_7f91',
    intervalMs: 3000,
    lastSync: '12:42:18',
  }),
  actions: {
    toggle() {
      this.paused = !this.paused;
    },
    async advance() {
      if (liveApi) {
        const page = await pollEvents(this.afterId);
        if (page.next_after_id) this.afterId = page.next_after_id;
        this.nextCursor = page.next_after_id ?? this.afterId;
        this.lastSync = new Date().toLocaleTimeString('zh-CN', { hour12: false });
        return;
      }
      const value = Number.parseInt(this.afterId.replace('evt_', ''), 10) + 1;
      this.afterId = `evt_${String(value).padStart(6, '0')}`;
      this.lastSync = new Date().toLocaleTimeString('zh-CN', { hour12: false });
    },
  },
});
