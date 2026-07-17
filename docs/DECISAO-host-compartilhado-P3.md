# Decisão pendente — host compartilhado serve quais tenants? (P3)

**Status:** aguardando decisão do usuário (levantada 2026-07-17 madrugada, no /loop noturno). Bloqueia a Task 5 do P3-backend (escolha de domínio no criar-link) e define o modelo de serving cloud.

## O problema

O redirect no **host compartilhado** (`quarkus.com.br`, `domain_id 0`) hoje escopa a leitura pra `DEFAULT_TENANT` (tenant 0), porque `links` está sob `FORCE ROW LEVEL SECURITY` (P2a) e a política é `tenant_id = current_setting('app.tenant_id')`. Resultado: um link de um **tenant cloud** (tenant_id != 0) dá **404 no host compartilhado** — só serve no domínio custom dele.

Isso contradiz o modelo travado no P1a ("domínio padrão compartilhado + domínio próprio opcional; **todos usam por padrão o domínio compartilhado**"). Um tenant cloud SEM domínio custom não teria onde servir seus links.

**Não é bug hoje** porque o `create_link_core` ainda carimba TODO link como `DEFAULT_TENANT` (`src/api.rs:~591`, placeholder). Vira bug real assim que a Task 5 carimbar o tenant do criador (necessário pro isolamento admin: um tenant só deve ver os próprios links).

A Task 4 (isolamento) está **correta e mergeável** — review Opus GO. O isolamento em domínio custom e no cache está fechado. Essa decisão é sobre o comportamento do host COMPARTILHADO, ortogonal ao isolamento já entregue.

## Tensão de fundo

`FORCE RLS` em `links` (P2a, defense-in-depth pro admin) vs. redirect público no host compartilhado que precisa ler QUALQUER tenant por id global. Sob FORCE, uma leitura no pool pelado (sem `app.tenant_id`) retorna 0 linhas. Short links **são públicos** (qualquer um com o código resolve) — ler um link por id no redirect não é vazamento de confidencialidade; o que precisa isolar é o ADMIN (listar/editar/deletar) e o BRANDING do domínio custom (o domínio do tenant A não serve link do B).

## Opções

**A) Shared = superfície do tenant 0 (default/OSS) só.** Cloud tenants servem só nos domínios próprios (custom, ou um subdomínio `tenant.quarkus.com.br` — modelo Vercel `projeto.vercel.app`). Simples, mantém FORCE RLS intacto. Custo: exige que todo tenant cloud tenha um domínio/subdomínio; o host compartilhado não é multi-tenant.
- Sub-variante A': dar a cada tenant um **subdomínio automático** do host compartilhado (`slug.quarkus.com.br`) que resolve como um domínio verificado implícito → some a necessidade de custom pra funcionar. Bom UX, encaixa no HostRouter.

**B) Shared = superfície global (estilo bit.ly).** O host compartilhado serve link de qualquer tenant por código global. Exige um caminho de **leitura pública por id cross-tenant** que contorne o FORCE RLS de `links` só pra o redirect: função `SECURITY DEFINER` no Postgres OU um método `get_link_public(id)` numa conexão com bypass, mantendo o admin RLS-scoped. Fiel ao "todos usam o compartilhado", mas mexe no modelo de segurança (revisar com cuidado).

## Recomendação (minha, pra validar)

**A' (subdomínio automático por tenant no host compartilhado)** como default + custom domain opcional. Cada tenant cloud ganha `slug.quarkus.com.br` tratado como domínio verificado implícito (o HostRouter resolve pelo slug → tenant). Assim:
- não mexe no FORCE RLS de `links` (cada subdomínio tem tenant conhecido → leitura tenant-scoped normal);
- todo tenant funciona de cara, sem precisar configurar domínio;
- o isolamento da Task 4 (owned_by por host) já cobre isto de graça;
- `quarkus.com.br` puro (sem subdomínio) fica sendo a superfície do tenant 0 / operador.

Se preferir o modelo bit.ly puro (um domínio único pra todos), é a opção B (com o custo de uma leitura pública que fura o RLS — implementável, mas quero seu aval por ser security-sensitive).

## Enquanto isso (no /loop)

Task 4 aceita (GO). Sigo com Tasks 6 (`/admin/domains`+verify) e 7 (wellknown/SSRF), independentes. **Task 5 fica pendente desta decisão.** Depois passo pro resto do P2 (LUC-23, LUC-25) pra não desperdiçar a noite.
