# LUC-44 v1 — Honrar Sec-GPC + PRIVACY.md

Data: 2026-07-18
Issue: LUC-44. Baseado em `docs/research/2026-07-18-luc44-gdpr-consent.md`.

## Decisões do dono (2026-07-18)

- Escopo do v1: **PRIVACY.md + honrar `Sec-GPC`**. Retenção configurável e
  erasure ficam pra uma fase 2 (fora deste escopo).
- `Sec-GPC`: **default-on** (honrado sempre, sem flag; um kill-switch pode
  entrar depois se preciso — YAGNI agora).
- Alcance do GPC: **suprime captura de analytics E conversion forwarding**.

## Contexto (confirmado no código)

A analytics do quark é server-side e cookieless: o redirect (`GET /:code`)
não seta cookie de tracking no visitante. O `ClickEvent` é enviado por
`st.analytics_tx.try_send(ev)` (`src/api.rs:1362`); esse mesmo evento, ao ser
drenado pelo worker, alimenta (a) a gravação no sink de analytics e (b) o
conversion forwarding pros pixels (GA4/Meta). Ou seja, **suprimir esse envio
corta analytics E forwarding de uma vez**.

O único cookie no visitante é o `qk_pw_<code>` (unlock de senha) — funcional,
isento; não é afetado.

## Design

### 1. Honrar `Sec-GPC` no redirect (`src/api.rs`, handler de `/:code`)

Ler o header uma vez (barato, no hot path):
```rust
let gpc = headers
    .get("sec-gpc")
    .and_then(|v| v.to_str().ok())
    .map(|v| v.trim() == "1")
    .unwrap_or(false);
```
Gate no envio do evento (linha ~1362):
```rust
if !gpc {
    let _ = st.analytics_tx.try_send(ev);
}
```
- `bump_visits` (enforce de `max_visits`) e o próprio redirect 302 **não** são
  afetados: contador de visita é funcional, não tracking.
- O webhook `link.clicked` (notificação first-party pro operador, `emit`
  gated por `clicked_subscribed`) **fica** — é entrega ao endpoint do próprio
  operador, não tracking de terceiro nem analytics do visitante. Documentar
  essa escolha no PRIVACY.md e num comentário no código.
- Sem config/flag: GPC é sempre honrado (default-on).

### 2. `docs/PRIVACY.md` (+ `docs/PRIVACY.PT_BR.md`)

Doc de privacidade pro operador, seguindo o formato dos docs (cabeçalho de
troca de idioma, prosa direta, sem em-dash). Conteúdo (do research doc):
- O que o quark captura no clique (server-side, sem cookie/JS no visitante):
  país, cidade, referer, User-Agent, timestamp; buffer de 1000 eventos/link.
- IP e `fbc` são `#[serde(skip)]` — nunca vão pra disco; existem só em memória
  pro conversion forwarding (quando ligado).
- O cookie `qk_pw_<code>` de unlock de senha: funcional, assinado, 12h, escopo
  de 1 link.
- Cookies do painel (login OIDC/sessão do operador): first-party, fora do
  escopo de consentimento do visitante.
- **GPC:** quark honra `Sec-GPC` automaticamente — quando o visitante o envia,
  a captura de analytics e o conversion forwarding são suprimidos pra aquele
  clique (o redirect continua funcionando). O webhook operacional
  `link.clicked` (first-party) não é afetado.
- Responsabilidade do operador: base legal (legitimate interest / aviso de
  privacidade), região de dados (o operador escolhe onde roda), retenção.
- Nota de escopo: banner de cookie não é necessário pro modelo cookieless
  atual; retenção configurável/erasure são follow-up (fase 2).

## Testes (TDD)

Em `tests/api_it.rs` (padrão dos testes de redirect/analytics de lá):
- **Suprime com GPC:** um `GET /:code` com header `Sec-GPC: 1` retorna 302
  normal, mas NÃO registra analytics (checar via o endpoint de stats / sink:
  contagem permanece 0). Sem o header, o mesmo clique registra.
- Confirmar que o redirect (Location) e o `bump_visits`/expiry não mudam com
  GPC (um link com `max_visits` ainda conta a visita e esgota).

## Fora de escopo (fase 2, precisa de decisão futura)

Retenção configurável por tempo, purge/erasure por link, controle fino de
IP/UA no forwarding, postura de consentimento explícito do Meta CAPI.

## Critérios de aceite

- [ ] `Sec-GPC: 1` no redirect suprime analytics + forwarding (sem afetar 302,
      visита-count, expiry).
- [ ] GPC honrado por padrão (sem flag).
- [ ] `docs/PRIVACY.md` + `.PT_BR.md` criados, sincronizados, sem em-dash.
- [ ] Teste de integração do GPC verde; suíte + clippy + fmt verdes.
