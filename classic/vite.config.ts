import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';
import path from 'path';

// https://vitejs.dev/config/
export default defineConfig({
  plugins: [react()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  // Prevent vite from obscuring rust errors
  clearScreen: false,
  // Tauri expects a fixed port, fail if that port is not available
  server: {
    port: 3013,
    strictPort: false,
    watch: {
      // Tell vite to ignore watching `src-tauri`
      ignored: ['**/src-tauri/**'],
    },
    proxy: {
      '/api/3dbag': {
        target: 'https://api.3dbag.nl',
        changeOrigin: true,
        rewrite: (path) => path.replace(/^\/api\/3dbag/, ''),
      },
    },
  },
  // To make use of `TAURI_ENV_DEBUG` and other env variables
  envPrefix: ['VITE_', 'TAURI_'],
  optimizeDeps: {
    exclude: ['laz-perf'],
  },
  worker: {
    format: 'es',
  },
  build: {
    // Tauri uses Chromium on Windows and WebKit on macOS and Linux
    target: process.env.TAURI_ENV_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    // Don't minify for debug builds
    minify: !process.env.TAURI_ENV_DEBUG ? 'esbuild' : false,
    // Produce sourcemaps for debug builds
    sourcemap: !!process.env.TAURI_ENV_DEBUG,
    chunkSizeWarningLimit: 2048,
  },
});
