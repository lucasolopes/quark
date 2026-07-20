import { defineConfig, configDefaults } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: './src/test-setup.ts',
    // Playwright specs live in ./e2e and must not be collected by Vitest (they
    // use @playwright/test, not the Vitest runner).
    exclude: [...configDefaults.exclude, 'e2e/**'],
    // Multi-step userEvent flows (dialogs with typing + clicks) can exceed the
    // 5s default on a loaded machine; give them headroom so the suite is not
    // flaky under load.
    testTimeout: 20000,
  },
})
