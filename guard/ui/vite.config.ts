import { fileURLToPath, URL } from 'node:url';
import { defineConfig } from 'vite';
import vue from '@vitejs/plugin-vue';

export default defineConfig({
  plugins: [vue()],
  server: {
    proxy: {
      '/api': {
        target: process.env.VITE_GMV_API_PROXY_TARGET ?? 'http://127.0.0.1:8080',
        changeOrigin: true,
      },
      '/health': {
        target: process.env.VITE_GMV_API_PROXY_TARGET ?? 'http://127.0.0.1:8080',
        changeOrigin: true,
      },
    },
  },
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
});
