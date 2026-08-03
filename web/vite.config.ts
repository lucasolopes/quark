import { defineConfig, configDefaults } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'
import { existsSync } from 'node:fs'

// The `@ee` alias (LUC-19): points at `src/ee/` in the Enterprise edition and
// at the inert stub in `src/lib/ee-stub.tsx` in Community. The `existsSync` is
// what makes "delete `web/src/ee/` and it still builds" true rather than a
// promise: with the directory gone, the alias falls back to the stub even when
// VITE_QUARK_EE is set.
const eeDir = path.resolve(__dirname, './src/ee')
const eeEnabled = process.env.VITE_QUARK_EE === '1' && existsSync(eeDir)

// https://vite.dev/config/
export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
      '@ee': eeEnabled ? eeDir : path.resolve(__dirname, './src/lib/ee-stub.tsx'),
    },
  },
  test: {
    globals: true,
    environment: 'jsdom',
    setupFiles: './src/test-setup.ts',
    // Playwright specs live in ./e2e and must not be collected by Vitest (they
    // use @playwright/test, not the Vitest runner).
    // `src/ee/**` only runs in Enterprise mode (`npm run test:ee`), where the
    // `@ee` alias points at the real implementation instead of the inert stub.
    exclude: [...configDefaults.exclude, 'e2e/**', ...(eeEnabled ? [] : ['src/ee/**'])],
    // Multi-step userEvent flows (dialogs with typing + clicks) can exceed the
    // 5s default on a loaded machine; give them headroom so the suite is not
    // flaky under load.
    testTimeout: 20000,
  },
})
