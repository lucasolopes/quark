// Enterprise test mode (LUC-19).
//
// Reuses the base config and overrides two things: the `@ee` alias now points
// at the real implementation instead of the inert stub, and `src/ee/**` joins
// the collection.
//
// It deliberately avoids an environment variable. Setting `process.env` in an
// ESM module does not work here: imports are hoisted, so the base config would
// read the variable before the assignment runs, and `test:ee` would end up
// running exactly what `test` runs. Overriding the object is deterministic and
// behaves the same on Windows and in CI.
import path from 'node:path'
import { configDefaults } from 'vitest/config'

import base from './vite.config'

export default {
  ...base,
  resolve: {
    ...base.resolve,
    alias: {
      ...(base.resolve?.alias as Record<string, string>),
      '@ee': path.resolve(__dirname, './src/ee'),
    },
  },
  test: {
    ...base.test,
    exclude: [...configDefaults.exclude, 'e2e/**'],
  },
}
