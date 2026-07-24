# Design refresh — referência local (Quark DS v2)

Snapshot dos arquivos do projeto Claude Design **"Quark: Encurtador URL
open-source"** (`projectId 75266895-4e2c-4763-a74b-da9e3b99de02`), extraído em
2026-07-23 para servir de referência estável durante a implementação da spec
`docs/specs/2026-07-23-design-refresh-web-design.md`.

- `design-system-readme.md` — fundamentos do DS (voz, paleta, tipo, iconografia).
- `tokens/` — colors, typography, spacing, effects (CSS custom properties).
- `components/` — referência dos componentes do DS (`.jsx`/`.d.ts`). São
  **recriações cosméticas**, não código de produção: o port real vira TSX
  com Tailwind/CVA em `web/src/components/`. Nota: o `MeterBar.jsx` de
  referência anima `width`; o port de produção deve animar `transform:
  scaleX()` (performance).
- `assets/` — marca oficial (Feistel-crossing): mark, tile (favicon), lockup.
- `mock/Quark.dc.html` + `mock/support.js` — o mock completo e navegável
  (login, painel com 6 tabs, modal de criar link, landing fora de escopo).
  Abrir o HTML num browser com os dois arquivos lado a lado renderiza o
  mock interativo.

Fonte da verdade em caso de dúvida: o próprio projeto no claude.ai/design.
