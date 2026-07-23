# Repaginada do painel web — Quark DS v2 (design)

**Data:** 2026-07-23
**Estado:** aprovado no brainstorming, aguardando plano

## Objetivo

Aplicar o design system refinado do quark (o "Quark DS v2", desenhado no projeto
claude.ai/design) em **todo o painel web** (`web/`): tela de login, Shell e as
telas internas. Junto, uma passada de engenharia no front: TypeScript 7,
Google TS Style Guide, `.npmrc` com boas práticas e limpeza de dependências.

A **landing pública fica fora** deste ciclo (não existe repo dela ainda).

## Fonte da verdade do design

Projeto Claude Design **"Quark: Encurtador URL open-source"**
(`projectId 75266895-4e2c-4763-a74b-da9e3b99de02`), acessível via `DesignSync`.
Arquivos relevantes:

- `Quark.dc.html` — o mock completo e refinado. Estados: `isLogin`, `isApp`
  (tabs **Links, Analytics, Domains, Members, Tokens, Webhooks**), modal de
  criar link (`createOpen`). O estado `isLanding` existe no mock mas está fora
  do escopo.
- `readme.md` — fundamentos (voz, paleta, tipografia, iconografia, motion).
- `tokens/*.css` — `colors`, `typography`, `spacing`, `effects`, `fonts`.
- `components/*` — Button, Badge, Card, Input, StatCard, MeterBar, Terminal
  (cada um com `.jsx`, `.d.ts`, `.prompt.md`).
- `assets/quark-mark.svg`, `quark-tile.svg`, `quark-lockup.svg` — marca oficial
  ("Feistel-crossing").
- `screenshots/*.png` — referência visual por tela.

Na fase de estudo do plano, esses arquivos serão extraídos para
`docs/research/design-refresh/` no repo, para consulta local estável durante a
implementação (o `.jsx` do kit é recriação cosmética, serve de referência, não
de código de produção).

### Tokens principais (resumo)

- **Superfícies (dark, tema padrão):** ink `#0A0B0F`, ink-2 `#0C0D13`, panel
  `#131521`, panel-2 `#1A1D2B`, input well `#0E1018`.
- **Texto:** `#E8EAF2` body, `#F3F5FA` headings, `#8A90A2` muted, `#6B7180` dim.
- **Accent único:** plasma-lime `#C6F94E` (hover `#D8FF70`, dim `#8FD12E`,
  texto sobre lime `#0A0B0F`). Usado com parcimônia: ação primária, estados
  ativos, numerais de métricas, glow.
- **Sinal (dados/charts apenas):** cyan `#4ADEDE`, violet `#8B7CF6`, danger
  `#FF6B6B`.
- **Bordas:** hairline `rgba(255,255,255,.09)`, forte `.16`.
- **Light theme:** bg `#EEF0F4`, card `#FFFFFF`, card-2 `#F4F5F8`, texto
  `#12141C`, muted `#5E6472`, borda `rgba(0,0,0,.10)`.
- **Tipografia:** Space Grotesk (display/números, tracking até `-0.035em`),
  Hanken Grotesk (body/UI), JetBrains Mono (códigos, dados, eyebrows
  UPPERCASE `0.18em`). Escala: hero 60, h1 44, h2 40, h3 26, stat 40, lead 19,
  body 15, sm 13, mono 13.5, label 12, chip 10.
- **Radii:** 5/8/11/14/18 + pill. **Sombras:** `--shadow-card`,
  `--shadow-modal`, glow lime. **Motion:** `.15s/.3s/.5s`,
  `cubic-bezier(.4,0,.2,1)`, hover lift `translateY(-3px)`, sem bounces.

## Estado atual (gap)

- `web/src/index.css` já carrega um **Quark DS v1**: ink/panel/plasma-lime e
  radius já aplicados nas variáveis shadcn, dark + light. O v2 refina valores,
  adiciona os tokens que faltam (glow, washes, escala tipográfica completa,
  motion) e principalmente muda o **desenho das telas**.
- Fontes: as 3 famílias certas, mas via **CDN Google Fonts** no `index.html`;
  `@fontsource-variable/geist` sobrando no `package.json`.
- Stack: React 19 + Vite 8 + Tailwind 4 + shadcn (variante Base UI) + TanStack
  Query/Table + react-router 7 + recharts + sonner + next-themes + lucide.
  Mantida integralmente; nada de framework novo (avaliado Astryx da Meta:
  descartado por ser beta, motor StyleX incompatível com a base Tailwind e por
  competir com o DS próprio).
- TypeScript `~6.0.2`, typecheck/build via `tsc -b && vite build` (o mesmo
  comando roda no build do CF Pages).
- 17 rotas em `src/routes/`; testes Vitest (~30 arquivos) + Playwright e2e.

## Decisões

1. **Escopo:** login + painel completo. Landing fora.
2. **Cobertura:** as 6 telas do mock + login ficam **fiéis ao mock**; as 8
   telas sem mock (Extensions, ExtensionDetail, Import, LinkStats, Pixels,
   SsoDomains, Onboarding, AcceptInvite, OidcProvider) são **extrapoladas**
   com os padrões do DS.
3. **Abordagem:** restyle profundo sobre a base shadcn/Tailwind existente.
   Preserva lógica, queries, i18n, auth e testes; muda tokens, variants e
   layout. Sem rebuild a partir do kit.
4. **TypeScript 7 (GA em 08/07/2026)** no lugar do 6.0: React/tsx não depende
   da API de compilador que ficou para o 7.1. Se `tsc -b` (project references)
   falhar no toolchain ou no CF Pages, fallback documentado: permanecer no 6.x
   e registrar o bloqueio.
5. **Google TS Style Guide** aplicado via regras automatizáveis no
   `.oxlintrc.json` (import type, naming, named exports em código novo, etc.)
   e correção das violações existentes. Sem reestruturar pastas.
6. **`.npmrc`** em `web/`: `save-exact=true`, `engine-strict=true` (com
   `engines` no `package.json` casando com `.node-version`), `fund=false`.
7. **Entrega:** branch única `feat/design-refresh`; merge na main só no fim
   (main tem auto-deploy, a troca visual em produção é atômica).
8. **Disciplina de customização:** cores/efeitos só via tokens (variáveis
   CSS), nunca hex solto em classe utilitária espalhada.

## Arquitetura da mudança

### 1. Fundação — tokens e fontes

- `index.css` vira a expressão canônica do DS v2: todos os tokens da seção
  acima, mapeados nas variáveis shadcn (`--background`, `--card`, `--primary`,
  `--muted`, `--border`, `--ring`, `--chart-*`, radii) para dark (padrão) e
  light (`.dark` variant atual mantida, `next-themes` continua o toggle).
- Fontes migram do CDN para **`@fontsource`** (Space Grotesk, Hanken Grotesk,
  JetBrains Mono; variable quando disponível), importadas no `main.tsx`.
  Remove o `<link>` do `index.html` e o pacote Geist.

### 2. Componentes

- Variants CVA dos `ui/*` ajustadas ao v2: button (fill lime + `--on-accent`,
  hover `#D8FF70` com lift, active `scale(.98)`), card (hover lift + borda
  lime 30% quando interativo), badge (pills, washes), input (well escuro),
  table (header panel-2, hairlines), tabs, dialog (scrim + blur), dropdown.
- Componentes novos portados do DS: **StatCard** (numeral display + label),
  **MeterBar** (barra com gradiente accent), **Terminal** (janela com
  traffic-lights, fonte mono), **PageHeader/eyebrow** (label mono UPPERCASE +
  heading display), todos em `src/components/` com testes.
- `QuarkMark` atualizado para o glifo Feistel-crossing oficial; favicon/tile
  atualizados a partir dos SVGs do projeto de design.
- Charts (recharts) recebem a paleta de sinal (lime/cyan/violet) via tokens.

### 3. Telas

- **Shell:** navegação e chrome seguem o mock do painel (`isApp`).
- **Fiéis ao mock:** Login, Links (+ modal criar link), Analytics, Domains,
  Members, Tokens, Webhooks.
- **Extrapoladas:** as 8 restantes, compostas com os mesmos padrões
  (PageHeader, cards, tabelas hairline, StatCard onde houver métrica).
- Nenhuma mudança de comportamento: rotas, chamadas de API, RBAC, i18n e
  fluxos permanecem; a mudança é visual/estrutural de layout.

### 4. Engenharia

- Upgrade TypeScript 7 + ajustes de config que ele exigir.
- `.npmrc` + `engines`.
- Regras do style guide no oxlint + correções.
- Limpeza: Geist, CDN de fontes, CSS morto do tema v1.

## Testes e verificação

- Testes Vitest existentes continuam valendo (comportamento intacto); onde o
  DOM mudar de estrutura, os testes são atualizados junto com a tela (no mesmo
  task, TDD).
- Componentes novos (StatCard, MeterBar, Terminal, PageHeader) nascem com
  teste.
- Gate por task e no fim: `npm run lint && npm run typecheck && npm test &&
  npm run build` verdes.
- Verificação visual final tela a tela contra `screenshots/*.png` do projeto
  de design (Chrome/Playwright), dark e light.
- E2e Playwright existente roda no fim (login OIDC incluso).

## Riscos

- **TS 7 x CF Pages/CI:** o build do Pages roda `tsc -b`. Mitigação: validar
  `npm run build` limpo localmente e o fallback da decisão 4.
- **Regressão visual em telas extrapoladas:** sem mock de referência, o risco
  é inconsistência; mitigado pela disciplina de tokens e review por task.
- **Testes acoplados ao DOM antigo:** custo de atualização absorvido task a
  task, nunca depois.

## Fora de escopo

- Landing pública (ciclo próprio, repo próprio).
- Mudanças de API/backend, comportamento, rotas ou i18n (além de strings
  novas que o layout exigir).
- Migração de biblioteca de componentes (Astryx ou outra).
