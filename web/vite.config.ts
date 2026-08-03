import { defineConfig, configDefaults } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'
import path from 'node:path'
import { existsSync } from 'node:fs'

// Alias `@ee` (LUC-19): aponta para `src/ee/` na edicao Enterprise e para o
// stub inerte em `src/lib/ee-stub.tsx` na Community. O `existsSync` e o que faz
// "apagar `web/src/ee/` e continuar buildando" ser verdade, e nao promessa: sem
// a pasta, o alias cai no stub mesmo com VITE_QUARK_EE ligado.
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
    // `src/ee/**` so roda no modo Enterprise (`npm run test:ee`), onde o alias
    // `@ee` aponta para a implementacao real em vez do stub inerte.
    exclude: [...configDefaults.exclude, 'e2e/**', ...(eeEnabled ? [] : ['src/ee/**'])],
    // Multi-step userEvent flows (dialogs with typing + clicks) can exceed the
    // 5s default on a loaded machine; give them headroom so the suite is not
    // flaky under load.
    testTimeout: 20000,
  },
})
