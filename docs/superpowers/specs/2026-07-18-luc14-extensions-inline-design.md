# LUC-14 — Extensões ativam inline (sem navegar pro Webhooks/Pixels)

Data: 2026-07-18
Issue: LUC-14. Relacionado: LUC-15 (removeu o seletor de kind da UI de Webhooks).

## Estado atual

`web/src/routes/Extensions.tsx`: catálogo de cards. `poweredBy: "webhooks"`
(Slack/Discord/Telegram/Zapier/Make/n8n) e `"pixels"` (GA4/Meta) só têm um
botão que navega pra `/webhooks` ou `/pixels`. `poweredBy: "sheets"` já ativa
inline (`SheetsAction`, connect/sync/disconnect). `"soon"` fica desabilitado.

## Design

Cada card passa a ativar a própria extensão via um **modal** aberto do card
(reusando as mutations já existentes), em vez de navegar. `SheetsAction`
inalterado; `"soon"` inalterado (desabilitado).

### Cards `poweredBy: "webhooks"`
Botão "Ativar" abre um `Dialog` com:
- Campo de URL (label/placeholder por canal: webhook URL do Slack/Discord/
  Telegram; endpoint do Zapier/Make/n8n).
- Seleção de eventos (checkboxes dos `WEBHOOK_EVENTS`), default TODOS marcados.
- Cria via `useCreateWebhook` (o mesmo hook do `Webhooks.tsx`) com `kind`
  **fixo por integração** (sem seletor, alinhado ao LUC-15):
  slack→`slack`, discord→`discord`, telegram→`telegram`,
  zapier/make/n8n→`generic`.
- Pós-sucesso: para `generic`, revelar o secret assinado (mesmo padrão do
  `Webhooks.tsx` — copiar o `whsec_...` uma vez); para os canais nativos, um
  toast de sucesso. Incluir um link discreto "Gerenciar em Webhooks".

### Cards `poweredBy: "pixels"`
Botão "Ativar" abre um `Dialog` reusando o formulário de criação de pixel do
`Pixels.tsx`, com o **provider fixo** (ga4/meta) e os dois campos de credencial
daquele provider (GA4: measurement_id + api_secret; Meta: pixel_id +
access_token). Cria via a mutation de criar pixel já existente. Link discreto
"Gerenciar em Pixels".

### Estado "configurado" (opcional, se barato)
Se der pra derivar de hooks já existentes (`useWebhooks`/lista de pixels) sem
custo extra relevante, mostrar um hint sutil ("N configurado(s)") no card. NÃO
é requisito; não introduzir queries novas caras só pra isso. O requisito é a
ativação inline.

## Escopo

- Reusar mutations/hooks existentes (`useCreateWebhook`, hook de criar pixel).
  NÃO refatorar `Webhooks.tsx`/`Pixels.tsx` num componente compartilhado se der
  pra montar um form compacto no próprio Extensions usando os hooks (evitar
  refactor grande). Extrair componente só se ficar claramente mais limpo.
- Backend intacto (webhooks aceitam `kind` de canal via API; pixels aceitam
  provider). Nada de novo endpoint.
- i18n EN + pt-BR pros textos novos. Sem em-dash.

Fora de escopo: novos conectores OAuth (LUC-9); mudar o backend.

## Testes

- vitest do componente (`Extensions.test.tsx` se existir, senão criar um
  mínimo): abrir o modal de um card webhooks e um pixels; submeter cria via a
  mutation com `kind`/provider corretos (mockar as mutations/queries como os
  testes existentes fazem). "soon" continua desabilitado.
- `npm test` + `npx tsc --noEmit` verdes.

## Critérios de aceite

- [ ] Cards webhooks (Slack/Discord/Telegram/Zapier/Make/n8n) ativam inline
      (modal), criando webhook com `kind` fixo por integração.
- [ ] Cards pixels (GA4/Meta) ativam inline com provider fixo.
- [ ] Sheets inalterado; "soon" desabilitado.
- [ ] Nenhum seletor de kind exposto (alinhado ao LUC-15).
- [ ] vitest + tsc verdes.
