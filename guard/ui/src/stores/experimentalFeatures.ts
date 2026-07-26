import { ref } from 'vue';
import { defineStore } from 'pinia';

const STORAGE_PREFIX = 'gmv.preview.experimental_features.';

export const useExperimentalFeaturesStore = defineStore('experimental-features', () => {
  const username = ref('');
  const enabled = ref(false);

  function sync(nextUsername: string | undefined, isAdmin: boolean): void {
    const normalized = nextUsername?.trim() ?? '';
    if (!normalized || !isAdmin) {
      username.value = '';
      enabled.value = false;
      return;
    }
    if (username.value === normalized) return;
    username.value = normalized;
    enabled.value = window.localStorage.getItem(storageKey(normalized)) === 'true';
  }

  function toggle(): boolean {
    if (!username.value) return false;
    enabled.value = !enabled.value;
    window.localStorage.setItem(storageKey(username.value), String(enabled.value));
    return enabled.value;
  }

  return { enabled, sync, toggle };
});

function storageKey(username: string): string {
  return STORAGE_PREFIX + username;
}
