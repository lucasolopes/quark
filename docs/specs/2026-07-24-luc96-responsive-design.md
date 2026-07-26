# LUC-96 — Painel responsivo (design)

**Data:** 2026-07-24
**Estado:** aprovado no brainstorming

## Objetivo

Todo o painel `web/` funcional e confortável em 360–430px (mobile), 768px
(tablet), 1024px (laptop) e 1440px+ (desktop, estado atual). O mock do Claude
Design não define mobile — as decisões abaixo são extrapolação consciente do
DS, aprovadas pelo usuário.

## Decisões de produto (aprovadas)

1. **Nav mobile = drawer + hambúrguer.** Corte único em `md` (768px):
   ≥ md mantém a sidebar fixa de 250px exatamente como hoje; < md a sidebar
   (e o trilho de ícones `w-16`, que fica aposentado) some e a navegação vive
   num drawer deslizante da esquerda.
2. **Topbar mobile enxuta**: `hambúrguer · lupa · [spacer] · +`. A lupa
   expande a busca em linha própria (autofocus + fechar), mesmo comportamento
   live/`?q=` de hoje. O "+" é o Novo link icon-only (aria-label). Idioma e
   tema migram para o rodapé do drawer no mobile.
3. **Dialogs**: os formulários grandes (Criar/Editar link) viram full-screen
   abaixo de `sm` (footer fixo alcançável); os pequenos (confirms, QR, token,
   convite...) continuam centrados com margem.

## Arquitetura

- **Drawer** (`web/src/components/MobileNav.tsx` ou equivalente): construído
  sobre o Dialog do Base UI (focus trap, Esc, scrim de graça), painel 280px
  `bg-sidebar` colado à esquerda, animação slide-in; conteúdo idêntico ao da
  sidebar (logo, WorkspaceSwitcher, nav groups com RBAC, connected, user
  card) + LanguageSwitcher e toggle de tema no rodapé. Fecha ao navegar.
  A fonte dos itens de nav é ÚNICA (o array `navGroups` do Shell é
  compartilhado, não duplicado).
- **Dialog full-screen**: prop `fullScreenOnMobile?: boolean` no
  `DialogContent` (`ui/dialog.tsx`) aplicando, abaixo de `sm`: `inset-0`,
  `h-dvh`, `max-w-none`, `rounded-none`, corpo com scroll e footer sticky
  (`border-t bg-card`). API existente intocada para os demais.
- **Cards/rows**: reflow por classes responsivas (o card de link vira coluna
  no mobile; filter rows e bulk bar com `flex-wrap`); tabelas reais recebem
  `overflow-x-auto` no próprio container (regra do DS).
- **Touch**: itens de nav do drawer e controles primários ≥ 44px de alvo em
  < sm (visual pode manter, alvo cresce via padding/min-height responsivos).

## Verificação

- Unit (Vitest): drawer abre/fecha/fecha-ao-navegar; busca expande/recolhe;
  classes do full-screen presentes quando a prop está ativa.
- **QA de scroll horizontal automatizado**: script Playwright local percorre
  as 15 telas × 4 breakpoints (360/768/1024/1440) × dark/light e FALHA se
  `document.documentElement.scrollWidth > window.innerWidth`.
- Revisão visual dos screenshots gerados pelo script (dono do processo).
- Gate por task: `npm run lint && npm run typecheck && npm test`.

## Fora de escopo

Mudanças de feature/API; PWA/offline; redesenho visual além do reflow.
