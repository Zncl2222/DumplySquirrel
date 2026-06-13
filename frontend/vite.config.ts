import { defineConfig, loadEnv } from 'vite';
import react from '@vitejs/plugin-react';

const envDir = new URL('..', import.meta.url).pathname;

export default defineConfig(({ mode }) => {
  const env = loadEnv(mode, envDir, '');
  const devProxyTarget =
    process.env.VITE_DEV_PROXY_TARGET ??
    env.VITE_DEV_PROXY_TARGET ??
    env.DEV_LOCAL_FRONTEND_PROXY_TARGET ??
    (env.DEV_BACKEND_PORT ? `http://127.0.0.1:${env.DEV_BACKEND_PORT}` : undefined) ??
    'http://127.0.0.1:8000';

  return {
    plugins: [react()],
    server: {
      port: 5173,
      proxy: {
        '/api': devProxyTarget,
      },
    },
  };
});
