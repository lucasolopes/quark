// Modo de teste da edicao Enterprise (LUC-19).
//
// Reaproveita a config base e sobrescreve dois pontos: o alias `@ee` passa a
// apontar para a implementacao real em vez do stub inerte, e `src/ee/**` entra
// na coleta.
//
// Nao usa variavel de ambiente de proposito. Setar `process.env` num modulo ESM
// nao funciona aqui: os imports sao hoisted, entao a config base leria a
// variavel antes da atribuicao rodar, e o `test:ee` acabaria rodando exatamente
// a mesma coisa que o `test`. Sobrescrever o objeto e deterministico e funciona
// igual no Windows e no CI.
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
