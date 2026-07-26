# LUC-138 + LUC-139 — Exclusão de workspace e feedback na criação (design)

**Data:** 2026-07-26
**Estado:** aprovado no brainstorming
**Issues:** LUC-138 (High), LUC-139 (Medium)

## Objetivo

Achados na verificação e2e da v0.4.0. Criar um workspace pelo painel funciona,
mas não existe caminho de volta (`DELETE /admin/tenants/:id` responde 404 e o
painel não expõe ação nenhuma), e a criação fica em "Criando…" por bastante
tempo sem dizer o que está acontecendo.

As duas entram juntas porque são a mesma tela e o mesmo fluxo de
provisionamento no Keycloak.

## Correção de premissa registrada na LUC-141, que vale aqui também

O levantamento deste ciclo desmentiu duas coisas que estavam escritas nas
issues ou assumidas:

- O papel somente-leitura chama **`Viewer`**, não `Reader`.
- `src/keycloak/client.rs` **não tem nenhum método de delete ou disable de
  realm**. Só os cinco `ensure_*`. O helper `admin_delete` existe, mas é usado
  apenas para tirar um usuário de um grupo. Excluir realm é código novo.

Some-se a isso que **não há precedente de soft-delete em lugar nenhum do
repo** (`deleted_at`, `is_deleted`, `archived`: zero ocorrências), e que
`Owner` e `Admin` hoje têm escopos idênticos (`role_scopes` devolve
`Scope::Full` para ambos; o comentário em `src/tenant.rs:104-106` diz que
distinguir gestão do tenant é responsabilidade da camada de handler).

## Decisões de produto (aprovadas)

1. **Hard delete.** Apaga tudo na hora, irreversível. É o que todo delete do
   repo já faz, e soft-delete exigiria filtrar por "não excluído" em toda
   consulta escopada por tenant, o que é superfície de bug de isolamento.
2. **O realm do Keycloak é apagado.** Coerente com o hard delete: não deixa
   recurso alocado nem polui a lista de realms.
3. **Só o `Owner` exclui.** Como `Owner` e `Admin` compartilham escopos, isso
   vira regra explícita no handler. O motivo de não dar ao `Admin`: esse papel
   vem do grupo do claim no IdP, então quem controla o Keycloak conseguiria se
   tornar `Admin` e apagar o workspace inteiro. `Owner` só nasce de criar o
   tenant ou de aceitar convite, e nunca é rebaixado por claim
   (`src/oidc.rs:681-683`).
4. **O último workspace não pode ser excluído.** Responde 409 com mensagem
   clara. O usuário nunca fica sem workspace e o `WorkspaceGate` não precisa de
   caminho novo.
5. **Confirmação por digitação do slug.** É a ação mais destrutiva do produto.
6. **Sem barra de progresso falsa na criação.** Não existe polling em lugar
   nenhum do painel, e fabricar etapas que avançam por timer é mentir. O que
   resolve o "parece travado" é dizer o que está acontecendo e avisar quando
   passar do normal.

## Parte 1 — Exclusão (LUC-138)

### O que precisa sumir

**Postgres.** As 18 tabelas de `TENANT_OWNED_TABLES`
(`src/store/postgres.rs:115-134`): `links`, `aliases`, `alert_rules`,
`link_health`, `sessions`, `webhooks`, `api_tokens`, `pixels`,
`wellknown_documents`, `click_counters`, `stats_meta`, `click_events`,
`webhook_deliveries`, `sheets_connection`, `domains`, `invites`,
`oidc_configs`, `sso_email_domains`. Mais **`memberships`**, que não está
nessa lista e precisa de tratamento à parte, e a linha de `tenants`.

`users` é global e **não** é tocada: um usuário pode ser membro de outros
workspaces.

**LMDB.** O range prefixado do tenant nos 12 sub-dbs de `TENANT_OWNED_DBS`
(`src/store/lmdb.rs:72`). Atenção: `sessions` **não** é prefixado por tenant no
LMDB, então a sessão sobrevive à exclusão ali. Na prática ela morre no primeiro
request, porque em modo cloud o `admin_guard` re-resolve a membership a cada
requisição e sem membership dá 403, mas a assimetria entre backends fica
registrada.

**ClickHouse.** `ALTER TABLE clicks DELETE WHERE tenant_id = ?`.

### Ordem das operações, e o que acontece se falhar no meio

A criação é best-effort: grava tenant e membership, provisiona o Keycloak, e o
201 sai mesmo se o Keycloak falhar, porque existe
`backfill_keycloak_provisioning` no boot para consertar. **A exclusão não pode
copiar esse desenho, porque não dá para backfillar um delete.**

Ordem escolhida: **transação no Postgres primeiro, Keycloak depois do commit.**

- Todas as tabelas, mais `memberships` e `tenants`, numa transação só. Ou some
  tudo, ou não some nada. Se o banco falhar, nada foi apagado e o usuário pode
  tentar de novo.
- Depois do commit, apaga o realm. Se isso falhar, o realm fica órfão e sai um
  `warn!` com o slug e o erro.

A ordem inversa seria pior: realm apagado com o tenant vivo é um workspace que
existe e no qual ninguém consegue entrar.

**Custo aceito e registrado: realm órfão é possível, e não há reaper.** Ao
contrário da criação, que tem backfill no boot. Se isso incomodar na prática, a
correção é um reaper que compara realms do Keycloak com slugs de `tenants`,
mas isso é escopo próprio e não entra aqui.

### ClickHouse não apaga na hora

`ALTER TABLE ... DELETE` no ClickHouse é **mutation assíncrona**: é aceita e
executada depois. Então "os cliques foram apagados" é verdade eventual, não
imediata. Isso precisa estar escrito na doc de usuário, porque é exatamente o
tipo de detalhe que vira discussão de retenção de dado depois.

### O slug volta a ficar livre

Consequência do hard delete, e é o oposto do que a issue temia: a linha de
`tenants` some e a constraint `UNIQUE` libera o slug; a linha de `domains` do
subdomínio some; e o realm de mesmo nome também. O slug pode ser reutilizado.

### Autorização

`DELETE /admin/tenants/:id` exige:

1. Sessão autenticada (não token de API: excluir workspace não é operação de
   automação).
2. Membership no tenant alvo com papel `Owner`. Qualquer outro papel: `403`.
3. O tenant alvo tem que ser um em que o usuário é membro; um id de outro
   tenant responde `404`, não `403`, para não vazar existência.
4. Se for o único workspace do usuário: `409`.

O endpoint é `404` em OSS (`!st.multi_tenant`), como os outros de workspace.

### Painel

Ação de exclusão no menu do workspace switcher, atrás de um diálogo que exige
digitar o slug. O botão de confirmar fica desabilitado até o texto bater.
Depois da exclusão, invalida `["me"]` e o `WorkspaceGate` leva o usuário ao
workspace restante.

O item de exclusão só aparece para quem é `Owner` do workspace atual, o que o
painel já sabe: `/admin/me` devolve `role` em cada membership.

## Parte 2 — Feedback na criação (LUC-139)

### Por que demora

Não é `INSERT`. `provision_tenant_keycloak` (`src/api/tenants.rs:71-141`) faz
realm, client, grupos e mapper, usuário owner e e-mail de senha, cada um uma
chamada HTTP à Admin API, com retry em `401`/`403` (o token precisa ser refeito
depois que o realm nasce, porque as roles de gestão dele só entram no composite
`admin` na criação). É legitimamente lento e não é problema a resolver: é passo
a comunicar.

### O que muda

- O texto do estado de carregamento diz o que está acontecendo, em vez de só
  "Criando…". O usuário precisa saber que há um login sendo preparado em outro
  sistema, senão a espera não faz sentido.
- Passado um limite, aparece uma segunda linha dizendo que está demorando mais
  que o normal e que a página pode ser recarregada com segurança.
- **Recarregar no meio passa a ter comportamento definido.** Hoje é indefinido.
  Como o tenant e a membership são gravados antes do Keycloak, um reload durante
  o provisionamento mostra o workspace já criado em `/admin/me`. O painel passa
  a tratar isso explicitamente em vez de por acaso.

Nada de etapas fabricadas por timer.

## Fora de escopo

- Transferir a propriedade de um workspace para outro usuário. É o caminho que
  um `Owner` que quer sair deveria usar, mas é feature própria.
- Exportar dados antes de excluir. Vale existir, mas não bloqueia a exclusão.
- Reaper de realm órfão (ver acima).
- Notificar os outros membros de que o workspace foi excluído. Não existe canal
  de notificação no produto.

## Restrições do projeto

Nada toca o hot path do redirect. `codec.rs` e `permute.rs` intocados. Os
testes gated de Postgres rodam contra role **não-superusuária**, com o banco
recriado antes de cada binária (ver LUC-143).
