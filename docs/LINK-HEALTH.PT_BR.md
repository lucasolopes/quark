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

Num deploy com várias instâncias o checker ainda não tem coordenação entre nós,
então set `QUARK_HEALTH_CHECK_SECS` em **exatamente uma** instância. Se todas as
réplicas tivessem, cada uma sondaria todo destino e uma única quebra dispararia o
webhook uma vez por réplica. Um sweeper com lease compartilhado (que deixa toda
instância manter a env) fica pra um refinamento futuro.

## Limites

- Uma sonda por varredura por link; uma falha transitória vira o link pra
  quebrado e a próxima varredura recupera (as duas transições emitem seu evento).
- A cadência é global; não há intervalo por link nem opt-out.
- Eventos de saúde são best-effort in-memory, como `link.clicked`/`link.expired`.
- O checker roda numa instância (veja Configuração); um lease entre nós fica pra
  um refinamento futuro.
- A sonda resolve o host do destino e se recusa a contatar endereços internos,
  de loopback ou link-local, então um nome público apontando pra um IP interno
  não é sondado (guard de SSRF).
- O filtro "só quebrados" é aplicado por página. Numa conta com muitos links
  onde os quebrados são raros, "Carregar mais" pode buscar páginas sem nenhum
  link quebrado antes de chegar neles; siga carregando pra paginar.
