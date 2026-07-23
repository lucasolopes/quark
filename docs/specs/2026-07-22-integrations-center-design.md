# Central de integracoes (redesign da aba Extensoes)

Design da evolucao da aba **Extensoes** de um "lancador/catalogo" (que hoje so
roteia pro Webhooks/Pixels generico) para uma **central de integracoes** de
verdade: catalogo, view dedicada por integracao, estado conectado/off por
integracao, e conexao via formulario ou via OAuth "install" que autoconfigura.

Nao inclui codigo. E o insumo de produto pra decidir escopo e fases.

## Problema (estado atual)

`web/src/routes/Extensions.tsx` e um catalogo curado onde cada card tem um
`poweredBy` (`webhooks | pixels | sheets | soon`) e ao "ativar" abre um dialog
inline OU navega pra tela generica de Webhooks/Pixels. Nao existe uma entidade
de **conexao** de primeira classe:

- Cards de webhook (Slack, Discord, Telegram, Zapier, Make, n8n) e de pixel
  (GA4, Meta) nao mostram estado "conectado/off" no card.
- So o **Google Sheets** tem ciclo de vida real (conectado, e-mail, link da
  planilha, ultimo sync, `last_status.state=error`, desconectar). Ele e o
  prototipo do redesign inteiro: generalizar o formato dele.

Resultado: a aba parece um proxy do Webhooks, nao uma central de integracoes.

## Modelo alvo

Uma entidade **Connection** generica + um **registro de conectores**
(metadata), com webhooks/pixels/sheets virando **drivers** por tras. Nao
reescreve os tres subsistemas; poe uma fachada na frente.

### Connection (por tenant)

```
Connection {
  id
  tenant_id          // isolamento por tenant e obrigatorio
  connector_id       // "slack", "ga4", "google_sheets" (FK pro registro)
  auth_type          // oauth | apikey | webhook
  status             // connected | error | expired | disabled | pending (derivado, nao setado pelo usuario)
  account_label      // "acme.slack.com", "GA4 - Marketing (G-XXXX)", e-mail
  config             // JSON de campos NAO-secretos (measurement_id, canal, url da planilha)
  secret_ref         // ponteiro pro cofre cifrado (nunca o segredo cru; nunca volta pro front)
  scopes             // OAuth
  expires_at         // expiracao do token (OAuth)
  last_health_at     // ultimo check/entrega/sync com sucesso
  last_error         // { code, detail, at } -> dispara o banner de "off"
  created_at, updated_at
}
```

`status` e **derivado** (da ultima entrega/sync/refresh), nunca setado a mao. O
unico estado que o usuario controla e `disabled` (um toggle).

### Registro de conectores (mais config do que codigo)

Uma tabela estatica de definicoes por `connector_id`, declarativa, pra que
adicionar um conector seja config e nao handler novo:

```
ConnectorDef {
  id, display_name, icon, brand_color, category
  auth_type: apikey | oauth | webhook
  config_fields: [ { key, label, type: text|secret, required, help_url } ]   // apikey/webhook
  oauth: { authorize_url, token_url, scopes, client_id_ref, callback_path, auto_provision }  // oauth
  health_check: { kind: probe | last_delivery | last_sync | token_expiry, ... }
}
```

Hibrido: registro declarativo pra 90% dos casos (form, params de OAuth) + um
hook opcional por conector pro health probe e pro auto-provision de OAuth
(ex.: Slack devolvendo a webhook URL no callback precisa de um handler
pequeno). Bespoke-por-provider e o anti-padrao (e onde o quark esta hoje, com
tres subsistemas paralelos).

## Tres tipos de connect (e o mapeamento do catalogo do quark)

O tipo e ditado pela API do provedor, nao e escolha livre.

| Tipo | UX | quark hoje |
|---|---|---|
| **OAuth install** | clica "Conectar" -> redirect -> consent -> callback grava token. Muitas vezes zero campos. | Google Sheets (ja e). Candidatos: Slack ("Add to Slack"), Notion. |
| **API-key / form** | usuario cola segredo(s) num form; valida e guarda. | GA4 (`measurement_id`+`api_secret`), Meta CAPI (`pixel_id`+`access_token`). TikTok/LinkedIn (futuros) iguais. |
| **Webhook URL** | usuario cola a URL de destino; pode receber um segredo de assinatura. | Zapier/Make/n8n (catch-hook + HMAC), Discord/Telegram (incoming webhook). |

Mapa dos primitivos existentes pro modelo novo:

| Conector | auth_type | Driver | config | secret | health |
|---|---|---|---|---|---|
| Slack/Discord/Telegram | webhook (Slack pode virar oauth) | webhooks | url, eventos, kind | - | ultima entrega |
| Zapier/Make/n8n | webhook | webhooks | url, eventos | segredo HMAC | ultima entrega |
| GA4 | apikey | pixels | measurement_id | api_secret | ultimo forward / probe no save |
| Meta CAPI | apikey | pixels | pixel_id | access_token | ultimo forward |
| Google Sheets | oauth | sheets | url da planilha, e-mail | access+refresh token | last_sync (ja existe) |

## Health: como "conectado" vira "off"

- **OAuth**: falha de refresh / revogacao -> `expired`/`error` -> banner
  "reconectar" (linguagem do Zapier: "needs reconnection" + botao Reconnect).
- **Webhook/sync**: status da ultima entrega/sync (o Sheets ja faz a versao
  pequena: `last_sync` + `last_status`). Falhas seguidas viram erro.
- **API-key**: valida no save; opcionalmente re-prova periodicamente (chamada
  autenticada barata). Em geral aparece como erro de entrega, nao probe.

Regra: preferir sinais **passivos** (resultado da ultima entrega/sync) a
polling ativo. Reconectar **preserva o id da conexao** pra que regras/pixels
que apontam pra ela continuem funcionando.

## View dedicada por integracao

Clicar num card abre uma **rota dedicada** (`/extensions/:connectorId`),
renderizada a partir do registro:

- **Nao conectado**:
  - `oauth` -> botao unico "Conectar [Provedor]" -> redirect (reusa o fluxo do
    Sheets: `sheetsConnect()` -> `window.location.href = url`). Sem campos antes
    do consent; se precisar config, coleta depois do callback.
  - `apikey` -> form declarativo (GA4/Meta hoje) com links de ajuda ("onde acho
    isso?") e validacao no save.
  - `webhook` -> URL + seletor de eventos; revela o segredo de assinatura uma
    vez pros kinds genericos.
- **Conectado**: pill verde, `account_label`, resumo da config, linha de health
  ("sincronizado ha 3 min" / "ultima entrega 200") e acoes **Reconectar / Editar
  / Desabilitar / Desconectar**. E o card conectado do Sheets promovido a view
  cheia e generica.
- **Erro/expirado**: pill ambar/vermelho + banner no topo com o motivo e um
  clique pra **Reconectar** (OAuth) ou **Atualizar credenciais** (apikey).
- **O card do catalogo** ja mostra a pill de status (o maior upgrade sobre hoje:
  ver "off" sem abrir o card).

## OAuth install que autoconfigura

O padrao premium (clica -> consent -> tudo pronto, zero campos):

- **Slack incoming-webhook OAuth**: `scope=incoming-webhook`; no consent o
  usuario escolhe o canal e o Slack **devolve a webhook URL no callback**.
  Maior ganho de UX (hoje o usuario cola a URL na mao).
- **Vercel/GitHub App**: o provedor dirige o ciclo (install cria uma
  installation, segredos sincronizados; config vem como schema declarado
  DEPOIS do auth). Referencia pra o futuro.

Requer: app OAuth registrado (client_id/secret do quark, nao do tenant),
scopes, callback fixo, `state`/PKCE (CSRF), e um handler por provedor pra
traduzir o token em conexao.

## Plano em fases (nao reescrever; agregar primeiro)

**Fase 1 - Central + view dedicada, sobre o que ja existe (sem migracao):**
- `GET /connections`: read-model que **une** webhooks + pixels + sheets no
  formato generico (`connector_id`, `status`, `account_label`, `last_health`).
  Sem migracao de dados.
- Registro de conectores no front (metadata do catalogo atual).
- Card do catalogo mostra a **pill de status** vinda do read-model.
- Rota `/extensions/:id` com a **view dedicada** renderizada do registro
  (form/oauth/webhook), reusando os fluxos de create/connect que ja existem.
  Isso entrega a UX pedida (view por integracao, conectado/off) sem reescrever
  backend.

**Fase 2 - Slack OAuth "Add to Slack":** o ganho de UX premium (autoconfigura
a webhook URL). Precisa do app Slack + callback verificado.

**Fase 3 - Storage generico + health:** promover pra uma tabela `Connection`
de verdade + sinais de health (ultima entrega/sync, refresh de token) +
reconectar-in-place preservando o id.

**Fase 4 - Novos conectores como config:** Notion (OAuth), TikTok/LinkedIn
(API-key) entram quase so como metadata no registro.

## Riscos / gotchas

- **Verificacao do app OAuth (o maior).** Google marca apps com scope sensivel
  (Sheets e sensivel) como "unverified", com tela assustadora e **teto de
  usuarios** ate passar na verificacao (avaliacao de seguranca, privacy policy,
  homepage, re-verificacao anual). OK pra app interno de 1 workspace; bloqueia o
  cloud multi-tenant publico ate verificar. Slack tem review mais leve.
  **Planejar o tempo de verificacao antes de prometer OAuth pra tenants
  externos.** (Ja ligado ao follow-up de GA da LUC-78.)
- **Storage de segredo:** nunca guardar o segredo cru na Connection nem devolver
  pro front; `secret_ref` -> cofre cifrado; campos write-only (mostra `****`,
  permite substituir). O quark ja faz isso com pixel/webhook/sheets.
- **Refresh/revogacao de token:** OAuth precisa de loop de refresh e detectar
  revogacao -> `expired` -> reconectar. Refresh token do Google **expira em 7
  dias enquanto o app esta em "testing"** (gotcha classico).
- **Isolamento por tenant:** Connection por `tenant_id`; o `state` do callback
  OAuth tem que amarrar tenant+usuario iniciador (single-use, expira em ~12h)
  pra um tenant nao completar o install de outro.
- **Client secret e do quark, nao do tenant:** um app serve todos; secret
  comprometido e incidente de frota. Rotacionavel.
- **Custo de health check:** preferir sinais passivos a polling; cachear status.
- **Back-compat:** webhooks/pixels/sheets tem que continuar funcionando;
  introduzir Connection como read-model agregador primeiro, migrar storage
  depois.

## Arquivos-chave (pra implementacao)

- `web/src/routes/Extensions.tsx` (catalogo + as tres actions bespoke; a action
  do Sheets e o template do estado conectado).
- `web/src/routes/Webhooks.tsx`, `Pixels.tsx` (telas de gestao que os cards hoje
  delegam).
- Backend: `src/webhooks/` (tipos + `delivery.rs` = fonte de health de entrega),
  `src/pixel.rs` (forwarding), e o conector OAuth do Sheets + endpoints
  status/sync/disconnect (a referencia de OAuth).
