# LUC-140 + LUC-141 — Redação da URL de webhook e falha permanente (design)

**Data:** 2026-07-26
**Estado:** aprovado no brainstorming
**Issues:** LUC-140 (Urgent), LUC-141 (High)

## Objetivo

Duas correções acopladas no subsistema de webhooks, achadas ao inspecionar os
logs de `quark-prod` depois da v0.4.0:

1. A URL de destino é a credencial em Discord, Slack e Make, e hoje é impressa
   crua em 10 pontos do log.
2. Um destino que responde 404 ou 410 é retentado até esgotar o orçamento de
   tentativas, evento após evento, para sempre. É esse loop que faz a
   credencial ser reimpressa a cada poucos segundos.

As duas entram na mesma branch porque a segunda é o que dá volume à primeira.

## Correção de premissa registrada na LUC-141

A issue dizia que o auto-disable seria "do jeito que o link checker já faz com
link quebrado". **Esse precedente não existe.** `src/health.rs` faz probe,
grava `LinkHealth` e emite `LinkBroken`/`LinkRecovered` na transição; nunca
desativa nada. Não há no repo um único caso de algo desligado automaticamente
por falhas repetidas. O mecanismo desta spec é novo, e foi desenhado como tal.

## Decisões de produto (aprovadas)

1. **Só 404 e 410 são permanentes.** 400 e 422 ficam de fora de propósito: um
   422 normalmente diz que o *nosso* payload está errado, e desativar a
   integração do cliente por bug nosso é o pior desfecho possível. 429, 5xx,
   timeout e erro de transporte seguem transitórios com o backoff atual.
2. **Uma tentativa de confirmação antes de desativar.** A primeira resposta
   permanente não desativa sozinha: agenda uma nova tentativa pelo mecanismo
   de backoff que já existe. Se a segunda também vier permanente, aí sim
   desativa. Isso absorve o 404 momentâneo de janela de deploy ou proxy sem
   reintroduzir o loop, porque o orçamento cai de 8 para 2.
3. **Coluna `disabled_reason` nova.** Deriva no frontend a partir de
   `!active && status == error` foi descartada: ela mente quando o usuário
   pausa na mão um webhook que já vinha falhando.
4. **Sem notificação ao dono.** A LUC-141 pedia avisar quem configurou o
   webhook. Não existe canal de notificação no produto (sem e-mail, sem
   in-app). O aviso é o estado no painel. Notificação de verdade é escopo de
   outra issue.
5. **Rotação de credencial fora do aceite da LUC-140.** `quark-prod` é
   ambiente de teste ainda não lançado, e a rotação está numa task própria do
   lançamento (registrado em comentário na issue).

## Parte 1 — Redação da URL (LUC-140)

### O problema é conhecido pelo código

`src/webhooks/delivery.rs:437-442` já traz um comentário explicando que o token
de canal vive na URL e que o `Display` do `reqwest` o inclui. A redação
(`e.without_url()`) foi aplicada **só** ao detalhe persistido em `HealthStatus`,
deixando o `tracing` de fora. Ou seja: corrigir os 10 sítios na mão não é a
correção, é repetir a mesma decisão parcial.

### Os 10 sítios

Todos em `src/webhooks/delivery.rs`, e nenhum fora dele. A varredura confirmou
que `pixel.rs` já aplica `without_url()` no erro tipado, e que `sheets/`,
`slack.rs` e `webhooks_api.rs` não logam URL.

| Linha | Macro | Contexto |
|---|---|---|
| 312 | `warn!` | url inválida (worker em memória) |
| 317 | `warn!` | bloqueado pelo guard de SSRF |
| 365 | `error!` | falha ao assinar |
| 429 | `warn!` | resposta não-2xx |
| 443 | `warn!` | falha de transporte |
| 466 | `warn!` | orçamento de tentativas esgotado |
| 632 | `warn!` | url inválida (relay durável) |
| 638 | `warn!` | bloqueado pelo guard de SSRF (relay) |
| 726 | `warn!` | não-2xx (relay) |
| 732 | `warn!` | falha de transporte (relay) |

### A solução: tornar o vazamento um erro de compilação

Newtype `WebhookUrl(String)` em `src/webhooks/mod.rs`, usado como tipo do campo
`WebhookSubscription::url`:

- `Display` e `Debug` **redigidos**: imprimem host mais id da subscription, no
  formato `discord.com/…#42`. Nunca o path, que é onde mora o token.
- `#[serde(transparent)]`: o valor cru continua indo e voltando do storage e da
  API sem mudança de formato. Nenhuma migração de dado, nenhum quebra-galho de
  compatibilidade, painel inalterado.
- Valor cru só através de `.expose()`, nome escolhido para ecoar o
  `ExposeSecret` do `secrecy` que o repo já usa em `AppState::signing_key`.

O mecanismo que faz isso valer a pena: `reqwest` aceita `IntoUrl`, que não é
implementado para o newtype. **Todo ponto que monta a requisição para de
compilar** até chamar `.expose()`. Os 10 `url = %sub.url` seguem compilando e
passam a sair redigidos por construção. O vazamento deixa de ser questão de
disciplina de quem escreve o próximo `warn!`.

Superfície medida do campo fora de `delivery.rs`: 12 sítios
(`webhooks_api.rs:53,250,291,596,654`, `slack.rs:128,233`,
`webhooks/mod.rs:121,359,507`). É pequena o bastante para o newtype valer.

### Por que não `secrecy::SecretString`

`SecretString` bloqueia o `Display` inteiro, e a gente **quer** um `Display`
útil para diagnóstico: o operador precisa saber qual subscription falhou sem
abrir o banco. O newtype dedicado dá redação e identificação ao mesmo tempo.

### Testes

- Unitário de tipo: `Display` e `Debug` de uma URL do Discord com token não
  contêm o token, e contêm o host e o id. É a guarda barata que roda sempre.
- Integração no caminho real de entrega: instala um subscriber de `tracing` com
  writer em memória (`Arc<Mutex<Vec<u8>>>`) via
  `tracing::subscriber::with_default`, dispara uma entrega que falha e asserta
  que o token não aparece na saída capturada. **Não precisa de crate nova** —
  `tracing-subscriber` já é dependência do projeto.

## Parte 2 — Falha permanente (LUC-141)

### São dois caminhos, não um

| | `deliver_one` (memória) | relay durável (outbox) |
|---|---|---|
| Local | `delivery.rs:399-480` | `delivery.rs:587-706` |
| Tentativas | `DELIVERY_ATTEMPTS = 3` | `MAX_DELIVERY_ATTEMPTS = 8` |
| Backoff | 200ms · 2^n + jitter | 2s · 2^(n-1), teto 600s, + jitter |
| Desfecho | só loga (fail-open) | `mark_dead` na linha da entrega |
| Backend | LMDB e eventos de clique | Postgres |

O "8x para sempre" da issue é o relay. Os dois avaliam a resposta igual:
`is_success()` ou fracasso genérico, sem olhar o código.

### Mudança

Uma função de classificação em `delivery.rs`:

```
permanente  := 404 | 410
transitório := todo o resto (429, 5xx, timeout, transporte)
```

O orçamento de tentativas passa a depender da classificação:

- permanente: **2** tentativas (a original mais a de confirmação), nos dois
  caminhos
- transitório: 3 e 8 como hoje, inalterado

O agendamento da tentativa de confirmação reusa o backoff existente sem
mudança, então no relay ela cai em ~2-3s com jitter. A implementação é o teto
de tentativas virar função do status, não uma constante.

Esgotado o orçamento **permanente** (e só ele), a subscription é desativada:
`active = false` e `disabled_reason` preenchido. Esgotar o orçamento
transitório mantém o comportamento atual, que é registrar saúde e seguir.

### Esquema

`WebhookSubscription` ganha `disabled_reason: Option<String>`, com
`#[serde(default)]` como os outros campos adicionados depois da v1.

O repositório **não tem diretório de migrations**: o schema Postgres evolui
inline em `src/store/postgres.rs` com `CREATE TABLE IF NOT EXISTS` e
`ALTER TABLE ... ADD COLUMN IF NOT EXISTS` no boot. A coluna nova segue esse
padrão, ao lado de `last_delivery_at`/`last_delivery_status`
(`postgres.rs:729-730`).

Método novo no trait `Store`, implementado nos dois backends:

```
disable_webhook(tenant, sub_id, reason) -> Result<(), StoreError>
```

No LMDB é uma leitura-modificação-escrita do registro da subscription, como
`record_webhook_health` (`lmdb.rs:555-569`). No Postgres é um `UPDATE` de
`active` e `disabled_reason` escopado por `tenant_id`.

### Painel

`web/src/routes/Webhooks.tsx` já deriva três estados em `webhookHealth()`
(`:85-89`): `paused`, `failing`, `active`. Entra um quarto, `disabled`, quando
`disabled_reason` está presente, com cor e rótulo próprios e o motivo exibido
junto do detalhe de erro que já é renderizado (`:293-296`). O toggle manual que
já existe (`:322-324`) continua sendo o caminho de reativar; reativar limpa
`disabled_reason`.

`disabled_reason` entra em `Webhook` em `web/src/lib/types.ts` e na resposta de
`GET /admin/webhooks` em `webhooks_api.rs:53`.

### Testes

- `410` é tentado duas vezes e desativa; `disabled_reason` fica preenchido.
- `404` idem.
- `404` na primeira e `200` na confirmação **não** desativa. Este é o teste que
  prova que a tentativa de confirmação serve para alguma coisa.
- `503` continua percorrendo o orçamento transitório inteiro e **não** desativa.
- Nos dois caminhos (`deliver_one` e relay), porque a política é duplicada.
- Frontend: o estado `disabled` renderiza com o motivo, e é distinto de
  `paused`.

## Fora de escopo

- Honrar `Retry-After` em 429. O comportamento atual (backoff genérico) é
  aceitável e ninguém reclamou dele.
- Notificação ao dono do webhook (ver decisão 4).
- Limpar as duas subscriptions mortas do ambiente de teste. Depois do deploy
  elas se desativam sozinhas na primeira falha permanente confirmada, que é
  justamente a validação em campo da mudança.

## Restrições do projeto

Nada aqui toca o hot path do redirect. `codec.rs` e `permute.rs` não são
tocados. O gate por task é `cargo fmt`, `cargo clippy --all-targets -D
warnings`, testes de lib e testes gated no Postgres não-superusuário.
