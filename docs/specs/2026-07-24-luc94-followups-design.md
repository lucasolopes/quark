# LUC-94 — Follow-ups do design refresh (design)

**Data:** 2026-07-24
**Estado:** aprovado no brainstorming
**Origem:** Minors acumulados nos reviews do LUC-93 (PR #4) + achados do QA
visual em produção. Nenhum item muda comportamento, exceto as três decisões
de produto abaixo.

## Decisões de produto (aprovadas pelo usuário)

1. **Tag chips do Links**: cap de 10 chips + chip "+N" que expande/recolhe o
   restante (botão acessível, estado local). Toda tag continua filtrável.
2. **Botão "Criar link" do PageHeader do Links**: removido. Ficam o botão
   global da topbar e o CTA do empty state (fiel ao mock).
3. **Avatares de membro**: rotação determinística só entre `--chart-2` e
   `--chart-3` (o vermelho `--chart-4` lia como estado de erro).

## Resoluções técnicas

- **i18n keys mortas (~20)**: remover das duas locales, com grep de segurança
  por key antes de cada remoção: `linkTable.columnCode/columnDestination/
  columnAlias/columnFolder/columnTags/columnCreated/columnExpires/
  columnVisits/caption/actionsSr/statsMenuItem`, `tokens.columnName/
  columnScopes/columnRateLimit/columnCreated`, `stats.heading`, `login.badge`,
  `pixels.measurementIdField`, `pixels.pixelIdField` e as keys `column*` de
  SSO/domains que o grep confirmar mortas.
- **`extensions.backAria`**: renomear para `extensions.backToExtensions`
  (é texto visível hoje; padrão `stats.backToLinks`), atualizando call sites.
- **`FIELD_LABEL_CLASS`**: export único em `web/src/lib/utils.ts` (ao lado do
  `cn`); os 4 arquivos duplicados importam de lá.
- **`OutOfShellFrame`**: novo `web/src/components/OutOfShellFrame.tsx` —
  backdrop (`bg-hero-glow` + `bg-dot-grid` aria-hidden), coluna central
  `max-w-[400px] animate-rise`, glifo com `glow-glyph`, título display 26px,
  subtítulo muted, `children` (o card), slot `topRight` (LanguageSwitcher do
  Login). Login/Onboarding/AcceptInvite migram sem mudança de comportamento.
- **`resolveShortHost` como fonte única**: `buildShortUrl` do LinkTable passa
  a derivar de `resolveShortHost` (protocolo aplicado por cima); a precedência
  primária→slug+sufixo→public→origin vive só em `web/src/lib/short-url.ts`.
- **Terminal**: `aria-hidden="true"` nos três traffic-lights decorativos.
- **`bg-dot-grid`**: adicionar `-webkit-mask-image` espelhando o `mask-image`.
- **Exit do dialog**: `@utility animate-rise-out` (espelho reverso da entrada,
  0.15s, mesmo cubic-bezier) aplicada via `data-closed:` em `dialog.tsx` e
  `alert-dialog.tsx`. O unmount do Base UI espera animações genericamente
  (verificado na T5 do LUC-93), então a saída anima sem travar o fechamento.
- **Switch do Webhooks (accname)**: `aria-label` passa a começar pelo texto
  visível — "Ativo — {url}" (o estado ligado/desligado vem do `aria-checked`
  do próprio switch; "Deactivate/Activate" sai do name).
- **Chart de cliques/dia**: `maxBarSize` no `<Bar>` (48) para poucos dias não
  virarem uma barra gigante.

## Verificação

- Gate por task e no fim: `npm run lint && npm run typecheck && npm test`.
- Testes atualizados junto de cada mudança (nunca enfraquecidos); itens novos
  com teste (chips +N, OutOfShellFrame smoke, fonte única do host).
- Entrega: branch `chore/luc94-design-refresh-followups`, PR no final.

## Fora de escopo

LUC-95 (CORS previews), qualquer mudança de API, novas features.
