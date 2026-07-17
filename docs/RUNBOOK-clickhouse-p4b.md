# Runbook — ClickHouse analytics sink (P4b, LUC-54)

O código do sink ClickHouse já está pronto (P4a, merge `af6068d`): schema, `ClickRow` com `tenant_id`, `ORDER BY (tenant_id, id, ts)`, `stats`/`stats_for_tenant`. Este runbook é a parte de **infra + validação**, que precisa de um servidor ClickHouse real — decisão e ação do operador.

> Contexto importante: o analytics **por tenant já funciona no Postgres** (backend de prod atual) via `GET /admin/stats`. O ClickHouse é **otimização pra volume alto**, não bloqueia o produto. Um único ClickHouse **particionado por `tenant_id`** (não uma instância por tenant).

## Como o app liga no ClickHouse

`open_backends` (`src/store/mod.rs:1009`) troca o sink de analytics de Postgres pra ClickHouse **quando `QUARK_CLICKHOUSE_URL` está setado** — senão usa o Postgres. É só analytics; links/redirect continuam no Postgres. Setou a env → `ClickHouseSink::open(url)` roda o `init_schema` (cria a tabela `clicks` com o `ORDER BY` correto se ela não existir).

## Decisão: onde hospedar (sua escolha)

| Opção | Prós | Contras |
|---|---|---|
| **Fly** (mesma infra do quark-prod) | uma conta só, rede interna Fly, custo previsível | você opera o ClickHouse (upgrades, backup, disco) |
| **ClickHouse Cloud** (gerenciado) | zero operação, escala/backup gerenciados | outra conta/fatura, egress cross-cloud |

Recomendação: se o volume ainda é baixo e você já opera o Fly, **Fly** mantém tudo num lugar; migre pro Cloud se a operação pesar. (Decisão de custo/operação — sua.)

## Passos

### 1. Provisionar
- **Fly:** subir um app ClickHouse (imagem `clickhouse/clickhouse-server`) com volume persistente, numa região próxima do `quark-prod` (gru). Expor só na rede interna Fly (`.internal`), com usuário/senha. Guardar a connection URL (`https://user:pass@host:8443` ou `http://…:8123`).
- **ClickHouse Cloud:** criar um serviço, pegar a URL HTTPS + credenciais.

### 2. Ligar no quark-prod
- Setar o secret no Fly: `fly secrets set QUARK_CLICKHOUSE_URL="<url>" -a quark-prod`.
- Deploy/restart. No boot, o `init_schema` cria a tabela `clicks` já com `ORDER BY (tenant_id, id, ts)`.

### 3. Validar contra o servidor real
Rodar a suíte gated (hoje pula sem servidor):
```
QUARK_TEST_CLICKHOUSE_URL="<url-de-teste>" cargo test --test clickhouse_sink_it
```
Confirma DDL, `record_batch` com `tenant_id`, e `stats`/`stats_for_tenant` com o predicado de tenant. **Use uma URL/servidor de TESTE** (a suíte cria/limpa a tabela), nunca o de prod com dados.

### 4. Backfill (só se já houver dados no ClickHouse)
Prod está vazio hoje, então **provavelmente não é preciso**. Se uma tabela `clicks` já tiver linhas com `tenant_id = 0` (pré-tenant), rodar um batch de mutations lendo o dono de cada `id` do Postgres:
```
ALTER TABLE clicks UPDATE tenant_id = <owner> WHERE id = <id>   -- async, em lote
```
(É o análogo do backfill que o Postgres já faz no boot. Código dedicado só vale a pena quando existir volume real; hoje seria não-testável.)

### 5. Caveat do `ORDER BY` (leia antes de mexer numa tabela existente)
ClickHouse **não muda o sort key via `ALTER`**. Uma tabela `clicks` **nova** nasce com `(tenant_id, id, ts)` (ótimo pros filtros por tenant). Uma tabela **pré-existente** (criada antes do P4a, com `(id, ts)`) **não** vira tenant-first por ALTER — precisa **rebuild**: criar `clicks_new` com o ORDER BY certo, `INSERT INTO clicks_new SELECT * FROM clicks`, `RENAME`/swap. Só relevante se você já tinha um ClickHouse rodando de antes; num servidor novo não se aplica.

## Rollback
Remover `QUARK_CLICKHOUSE_URL` e redeployar → o sink volta pro Postgres na hora, sem perda (o Postgres nunca deixou de ser a fonte dos links; o analytics volta a agregar no Postgres). Os dados já escritos no ClickHouse ficam lá pra quando religar.
