[English](LINK-HEALTH.md) · **Português**

# Monitoramento de link quebrado

O quark pode checar periodicamente se o destino de cada link ainda responde e
sinalizar os que quebraram. Você encurtou um destino meses atrás, ele caiu, e em
vez de uma reclamação de usuário você recebe uma notificação.

O checker vem desligado por padrão. Ele só roda quando você seta
`QUARK_HEALTH_CHECK_SECS` (o número de segundos entre varreduras), então nenhum
HTTP de fundo é feito sem você pedir.

## O que faz

A cada varredura, o checker percorre todos os links e sonda cada destino com uma
requisição `HEAD` (caindo pra `GET` se o servidor recusar HEAD). Uma resposta
`2xx` ou `3xx` conta como saudável; `4xx`, `5xx`, timeout ou falha de conexão
conta como quebrado. Ele não segue redirect: um `3xx` só quer dizer que o
servidor está vivo, e não seguir evita ser levado a um endereço interno.

Destinos em hosts internos ou de loopback nunca são sondados.

O resultado é guardado por link. No painel, uma bolinha em cada link mostra o
status (verde acessível, vermelho quebrado); um filtro "só quebrados" estreita a
lista. A API expõe isso como um objeto `health` em cada linha de link e um filtro
`?health=broken` (veja [API](API.PT_BR.md)).

Quando um link muda de estado, o quark emite um webhook: `link.broken` quando um
link saudável cai, `link.recovered` quando ele volta. Assine esses eventos em
[Webhooks](WEBHOOKS.PT_BR.md) (ou mande pro Slack/Discord/Telegram) pra ser
notificado. Um destino quebrado na primeira vez que é checado dispara
`link.broken` uma vez.

```mermaid
flowchart TD
    T[a cada QUARK_HEALTH_CHECK_SECS] --> L[lista todos os links]
    L --> I{host interno?}
    I -- sim --> S[pula, fica sem checar]
    I -- não --> P[HEAD, fallback GET]
    P --> C{2xx/3xx?}
    C -- sim --> H[saudável]
    C -- não --> B[quebrado]
    H --> W[grava saúde]
    B --> W
    W --> D{mudou de estado?}
    D -- saudável para quebrado --> EB[emite link.broken]
    D -- quebrado para saudável --> ER[emite link.recovered]
    D -- sem mudança --> N[fim]
```

## Configuração

| Variável | Efeito |
|---|---|
| `QUARK_HEALTH_CHECK_SECS` | Segundos entre varreduras. Sem setar, desliga o checker. Valores abaixo de 60 são elevados pra 60. |

Num deploy multi-nó a varredura roda só no nó designado (aquele cujo
`QUARK_NODE_ID` é `0` ou não setado), pra um destino não ser sondado uma vez por
nó. Set `QUARK_HEALTH_CHECK_SECS` nesse nó.

## Limites

- Uma sonda por varredura por link; uma falha transitória vira o link pra
  quebrado e a próxima varredura recupera (as duas transições emitem seu evento).
- A cadência é global; não há intervalo por link nem opt-out.
- Eventos de saúde são best-effort in-memory, como `link.clicked`/`link.expired`.
- O checker é single-node por design; distribuir a varredura entre nós fica pra
  um refinamento futuro.
