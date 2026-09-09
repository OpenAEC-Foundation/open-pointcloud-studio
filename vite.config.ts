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
      // Nothing generated should be watched: writing a large file into a
      // watched directory kills the dev server with EBUSY on Windows, and a
      // build or a fixture download does exactly that.
      ignored: [
        '**/src-tauri/**',
        '**/dist/**',
        '**/dist-debug/**',
        '**/dev-fixtures/**',
      ],
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
    // laz-perf ships CommonJS. Excluding it from pre-bundling hands the raw
    // CJS to the browser, where it dies on "exports is not defined" and takes
    // all LAZ support with it. Let esbuild convert it to ESM.
    include: ['laz-perf'],
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
