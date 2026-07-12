# Roadmap do quark

Estado atual (v0.1): núcleo em produção — criar + redirecionar + alias + expiração.
Único binário, sem dependências externas, deployado via Coolify. Testado (20 testes),
benchmarkado (permute ~22M ops/s; redirect de produção escalou linear até 1k VUs,
0 erro, latência dominada pelo RTT).

## Em andamento
- **Observabilidade** — métricas (req, latência, cache hit-rate) + logs estruturados.
- **Polir README** — badges, demo, estrutura pra vitrine open-source.
- **Edge/CDN** — cache de redirect na borda pra cortar latência de usuários distantes
  (o gargalo medido foi geografia, não o servidor).

## Próximos (planejado)
4. **Analytics de cliques** — contagem/timestamp, feito **async/batched** pra NÃO sujar
   o caminho quente do redirect (o GET tem que continuar voando).
5. **Contas + painel web** — login e UI pra gerenciar links (deixa de ser só infra e
   vira produto).
6. **Domínios customizados, QR code**, etc.

## Diferido (consciente)
- **Proteção contra abuso** (rate-limit no `POST /`, blocklist de destino) — não é
  necessário enquanto o acesso é privado; obrigatório antes de abrir criação ao público.
- **Escala horizontal** — o contador de IDs é single-node; múltiplas réplicas exigiriam
  particionar o espaço de IDs.
