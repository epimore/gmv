import { computed, ref } from 'vue';
import { defineStore } from 'pinia';
import { currentSession, login, logout, type SessionInfo } from '@/api/client';

export const useAuthStore = defineStore('auth', () => {
  const session = ref<SessionInfo | null>(null);
  const initialized = ref(false);
  let restorePromise: Promise<boolean> | null = null;

  const isAdmin = computed(() => session.value?.role === 'admin');

  async function restore(): Promise<boolean> {
    if (session.value) return true;
    if (restorePromise) return restorePromise;
    restorePromise = currentSession(false)
      .then((current) => {
        session.value = current;
        return true;
      })
      .catch(() => {
        session.value = null;
        return false;
      })
      .finally(() => {
        initialized.value = true;
        restorePromise = null;
      });
    return restorePromise;
  }

  async function signIn(username: string, password: string): Promise<void> {
    session.value = await login(username, password);
    initialized.value = true;
  }

  async function signOut(): Promise<void> {
    try {
      await logout();
    } finally {
      clearSession();
    }
  }

  function updateSession(current: SessionInfo): void {
    session.value = current;
    initialized.value = true;
  }

  function updateNickname(nickname: string): void {
    if (session.value) session.value = { ...session.value, nickname };
  }

  function clearSession(): void {
    session.value = null;
    initialized.value = true;
  }

  return { session, initialized, isAdmin, restore, signIn, signOut, updateSession, updateNickname, clearSession };
});
