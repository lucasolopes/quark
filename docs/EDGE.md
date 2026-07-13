# Edge caching com Cloudflare

## Por que isso ajuda

O quark resolve um redirect em microssegundos — o gargalo medido nunca foi o
servidor. É a **geografia**: cada `GET /:code` faz uma viagem de ida e volta
(RTT) até a instância única do Coolify, que está em uma região só. Um usuário
do outro lado do mundo paga esse RTT completo em toda clicada, mesmo que o
link nunca mude entre um clique e outro.

Colocar um CDN na frente resolve exatamente esse problema: o **302 em si**
(o par código→URL) pode ser cacheado na borda, perto do usuário. O clique
seguinte no mesmo link, vindo da mesma região, nem chega a bater no Coolify —
o CDN responde direto do cache. Isso não troca a lógica de redirect por nada
mais esperto; só evita repetir a viagem para uma resposta que já se sabe.

## O que o quark já manda (não precisa configurar nada nele)

Desde este commit, toda resposta de redirect carrega um header
`Cache-Control` calculado a partir do TTL do link (`src/api.rs`,
`cache_control_for`):

| Situação | Status | `Cache-Control` |
|---|---|---|
| Link sem TTL | 302 | `public, max-age=86400` (1 dia) |
| Link com TTL, ainda vivo | 302 | `public, max-age=<segundos até expirar>` (nunca passa de 86400) |
| Código/alias inexistente | 404 | `no-store` |
| Link expirado | 410 | `no-store` |

Ou seja: o CDN só precisa **respeitar o header que a origem já envia** — o
quark nunca deixa um 302 ser cacheado além do momento em que o link expira.
Não há configuração de TTL a duplicar na Cloudflare; a fonte da verdade é o
`Cache-Control` calculado pelo próprio handler.

## Passo a passo (Cloudflare, plano free)

1. **Domínio na Cloudflare.** Se o domínio já está lá, pule. Senão, adicione
   o domínio (`Add a Site`) e aponte os nameservers pra Cloudflare.
2. **Registro DNS do quark, proxied.** Crie (ou edite) o registro `A`/`CNAME`
   que aponta pro domínio que o Coolify te deu, e deixe o ícone da nuvem
   **laranja** (proxied), não cinza (DNS only). Só com proxy ligado a
   Cloudflare fica no meio do caminho e pode cachear.
3. **Confirme que a Cloudflare respeita o `Cache-Control` de origem.**
   No plano free, por padrão a Cloudflare só cacheia certas extensões de
   arquivo estático — HTML e respostas de API/redirect não entram nessa
   lista default. Para o 302 do quark ser cacheado, é preciso uma
   **Cache Rule** explícita (passo 4). Sem ela, o proxy ainda passa o
   tráfego (e já ajuda com TLS/anycast), mas não cacheia a resposta.
4. **Cache Rule para `/*`:**
   - Vá em **Caching → Cache Rules → Create rule**.
   - Condição: `Hostname equals <seu-dominio>` (ou `URI Path` `matches regex`
     `^/[A-Za-z0-9_-]+$` se quiser restringir à forma dos códigos).
   - Ação: **Cache eligibility → Eligible for cache**.
   - **Edge TTL: "Use cache-control header if present, bypass cache if not"**
     — esse é o ponto crítico. Isso faz a Cloudflare usar exatamente o
     `max-age` que o quark calculou, em vez de um TTL fixo da regra.
   - Deixe **Browser TTL** também respeitando o header de origem (não force
     um valor fixo aqui).
   - Salve e ative a regra.
5. **(Opcional) Respeitar `no-store`.** Verifique que a Cache Rule não tem
   nenhuma configuração que sobrescreva `no-store`/`no-cache` — o
   comportamento default de "usar o header de origem" já respeita isso, só
   não adicione uma regra separada forçando cache em `/health` ou em `POST /`.
   `POST /` (criação de link) não deve ser cacheado; a Cache Rule acima só
   precisa cobrir os `GET /:code`.

## O caveat do TTL curto

Um link criado com `ttl: 60` (60 segundos) gera automaticamente
`Cache-Control: public, max-age=<até 60>` — nunca mais que isso. Isso
significa que:

- Links de vida curta ganham **pouco benefício de cache de borda** (o CDN
  vai revalidar quase toda hora), mas isso é o comportamento correto: um
  link que expira em 30s não pode ficar servindo a URL antiga por 1 dia
  numa borda que não sabe da expiração.
- Links **sem TTL** (o caso comum) são os que mais se beneficiam: `max-age`
  de até 1 dia, cacheáveis com folga em qualquer borda próxima do usuário.

Não há nada a ajustar na Cloudflare por causa disso — o `max-age` já vem
certo da origem, regra por regra, link por link.

## Como purgar o cache se um link precisar mudar

O quark não expõe hoje um endpoint de "atualizar link" (a única forma de
matar um link antes da hora é deixá-lo expirar ou trocar o código). Se for
necessário invalidar um redirect que já está cacheado na borda antes do TTL
vencer:

1. **Purge por URL específica** (mais cirúrgico): Cloudflare dashboard →
   **Caching → Configuration → Purge Cache → Custom Purge** → cole a URL
   completa (`https://<dominio>/<code>`). Efeito quase imediato.
2. **Purge everything** (só se necessário, afeta tudo que está cacheado no
   domínio): mesmo menu → **Purge Everything**.
3. Via API (útil para automatizar): `POST /zones/:zone_id/purge_cache` com
   `{"files": ["https://<dominio>/<code>"]}` — ver
   [docs da Cloudflare](https://developers.cloudflare.com/cache/how-to/purge-cache/).

Depois do purge, o próximo `GET` naquele código vai até o Coolify de novo,
busca o valor atual e a Cloudflare recacheia com o `Cache-Control` (e portanto
o `max-age`) que o quark mandar naquele momento.
