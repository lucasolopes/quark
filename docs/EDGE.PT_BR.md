[English](EDGE.md) · **Português**

# Edge / CDN e caching de redirect

## Por que edge ajudaria

O quark resolve um redirect em ~2 ms: o gargalo medido nunca foi o servidor,
é a **geografia**: cada `GET /:code` faz um ida-e-volta (RTT) até a instância
única, que fica em uma região só. Um usuário do outro lado do mundo paga esse
RTT completo em toda clicada, mesmo que o link nunca mude.

## O que o quark manda (e isso já funciona)

Toda resposta de redirect carrega um `Cache-Control` calculado a partir do TTL
do link (`src/api.rs`, `cache_control_for`):

| Situação | Status | `Cache-Control` |
|---|---|---|
| Link sem TTL | 302 | `public, max-age=86400` (1 dia) |
| Link com TTL, ainda vivo | 302 | `public, max-age=<segundos até expirar>` (nunca > 86400) |
| Código/alias inexistente | 404 | `no-store` |
| Link expirado | 410 | `no-store` |

**Os browsers respeitam esse header.** Quando o mesmo usuário clica no mesmo
link de novo, o navegador dele serve o redirect **do cache local, sem tocar a
rede**. Esse ganho já funciona e está ativo agora: é por-usuário, não por-região.

## Realidade medida: a Cloudflare NÃO cacheia o 302

> Testado neste deploy (Cloudflare, plano free, atrás de Cloudflare Tunnel):
> mesmo com uma **Cache Rule** marcando o path como *Eligible for cache* **e**
> um **Edge TTL fixo forçado**, o `Cf-Cache-Status` permaneceu **`DYNAMIC`** em
> todas as requisições. A Cloudflare trata **redirects 3xx como dinâmicos** e
> não os coloca no cache de borda.

Ou seja: **não adianta criar Cache Rule pra cachear o 302**: não é
configuração errada, é comportamento da plataforma. Não gaste tempo nisso.

## Com Cloudflare Tunnel (nativo do Coolify)

Se você usa o `cloudflared` do Coolify (recomendado), o tráfego **já passa
pela borda da Cloudflare** por construção (confirmável pelo header `cf-ray` na
resposta), e **DNS + TLS ficam por conta do túnel**: nada de registro A
proxied, modo SSL ou certificado de origem pra configurar. Mas, pelo item
acima, a borda continua **não** cacheando o 302.

## Se você REALMENTE precisar de redirect cacheado na borda

O único caminho confiável na Cloudflare é um **Worker**: um script na borda que
ou (a) faz o próprio redirect lendo o par código→URL de um **Workers KV**, ou
(b) cacheia o 302 da origem via **Cache API**. Isso é uma mudança de fase 2:
exige levar os dados dos links pra onde o Worker alcança (dual-write
pro KV, ou o Worker consultando a origem e cacheando).

**Vale a pena?** Só se você tiver tráfego relevante **longe** da região da VPS.
Para um público próximo da origem (ex.: VPS na Europa + usuários europeus), o
RTT já é baixo e o Worker não compensa. Decisão deliberada atual: **não fazer**.
O `Cache-Control` já está pronto pra quando/se valer a pena.

## Resumo

- `Cache-Control` correto no 302 → **cache de browser funciona** (per-usuário). ✓
- Cache de **borda da Cloudflare para o 302** → **não funciona** (3xx é dinâmico). ✗
- Edge de verdade p/ redirect dinâmico → **Worker** (fase 2, ROI baixo se o
  público é perto da origem).
