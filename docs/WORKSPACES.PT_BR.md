[English](WORKSPACES.md) · **Português**

# Workspaces

Um workspace é um tenant: tem os próprios links, cliques, analytics, webhooks,
domínios, convites, tokens de API e realm de login. Nada atravessa de um
workspace para outro, e uma pessoa pode participar de vários e alternar entre
eles pelo painel.

Workspaces só existem no modo multi-tenant (cloud). Numa instância OSS de
operador único existe um tenant implícito e as rotas `/admin/tenants`
respondem `404`.

## Papéis

Toda membership tem um papel: `Owner`, `Admin`, `Member` ou `Viewer`. Hoje
`Owner` e `Admin` têm os mesmos escopos para tudo que é uso diário do
workspace, e a diferença aparece só onde a ação destrói o workspace.

`Owner` vem de criar o workspace ou de aceitar um convite que diga esse papel,
e nenhum claim de login promove alguém a `Owner`. Já o `Admin` vem do grupo que
chega no claim do provedor de identidade, ou seja, quem administra o Keycloak
consegue distribuir `Admin`. É por isso que a exclusão descrita abaixo é só do
`Owner`.

## Criar um workspace

Criar um workspace grava o tenant e a membership de dono, semeia o subdomínio
do tenant e depois provisiona o lado do login no Keycloak: realm, client,
grupos de papel com o mapper, o usuário dono e o e-mail de definição de senha.
Cada uma dessas etapas é uma chamada à Admin API do Keycloak, então a
requisição demora bem mais que um insert no banco. O painel diz o que está
esperando em vez de mostrar só "Criando…".

A linha do tenant e a sua membership são gravadas antes de o Keycloak entrar
na história, então recarregar a página no meio do caminho é seguro: o workspace
já aparece em `/admin/me`. Se alguma chamada de provisionamento falhar, um
backfill no próximo boot termina o realm.

## Excluir um workspace

`DELETE /admin/tenants/:id`, exposto no painel pelo menu do seletor de
workspace, atrás de um diálogo que pede para você digitar o slug do workspace.
O botão de confirmar fica desabilitado até o texto bater.

### A exclusão não tem volta

Não existe lixeira, arquivamento nem desfazer. Tudo que pertence ao workspace
some numa transação só de banco:

- os links e seus aliases, com as regras de redirect, as variantes e as senhas
  que eles carregam
- contadores de clique, eventos de clique e as estatísticas que a tela de
  analytics lê
- os registros de saúde de link e suas regras de alerta
- as assinaturas de webhook e o histórico de entrega
- os tokens de API
- os pixels de encaminhamento de conversão e os documentos well-known
  hospedados
- a conexão com o Google Sheets
- os domínios customizados e o subdomínio do tenant
- os convites pendentes
- a configuração OIDC do workspace e os domínios de e-mail de SSO
- as sessões ativas
- as memberships de todos os membros

Sua conta de usuário não entra nessa lista. Usuário é global, porque a mesma
pessoa pode participar de outros workspaces, então a conta continua existindo e
só perde a membership do workspace excluído.

Como é uma transação só, uma falha de banco deixa o workspace inteiro do jeito
que estava e você pode tentar de novo. Não existe estado pela metade.

### Só o Owner exclui

Um `Admin` recebe `403`, e o mesmo vale para `Member` e `Viewer`. A regra é
proposital, mesmo o `Admin` tendo os mesmos escopos do `Owner` em todo o resto:
o papel `Admin` vem de um grupo no claim do provedor de identidade, então quem
controla o Keycloak poderia se dar `Admin` e, se a regra fosse por escopo,
destruir o workspace com isso. `Owner` não se consegue por esse caminho.

Pedir a exclusão de um workspace do qual você não é membro responde `404`, do
mesmo jeito que um id que nunca existiu, então o endpoint não serve para
descobrir quais workspaces existem.

### O último workspace não pode ser excluído

Se o workspace for o único do qual você participa, o pedido é recusado com
`409` e nada é tocado. Ficar sem workspace nenhum não é um estado do qual o
painel saiba sair. Crie ou entre em outro workspace antes, ou deixe esse onde
está.

### O slug volta a ficar livre

Slug é único entre os workspaces que existem, não é reservado para sempre.
Depois da exclusão, a linha do tenant, o subdomínio e o realm do Keycloak
foram embora junto, então dá para criar um workspace novo com o mesmo slug na
hora. Esse workspace novo não herda nada do antigo além do nome.

### No ClickHouse os cliques somem depois, não na hora

Se a sua instalação guarda evento de clique no ClickHouse, a exclusão é enviada
como `ALTER TABLE clicks DELETE`, que o ClickHouse trata como mutation: o
comando é aceito na hora e executado em segundo plano. A API responde sucesso
no momento em que a exclusão foi aceita, então durante uma janela depois disso,
normalmente de segundos a minutos conforme a carga do cluster, parte das linhas
de clique do workspace excluído ainda existe fisicamente no ClickHouse.

Do produto essas linhas ficam inalcançáveis assim que o workspace some: não
sobra tenant para consultá-las nem login que chegue até elas. O que atrasa é a
remoção física em disco, e isso importa se o seu compromisso de retenção ou de
exclusão de dado estiver escrito em termos de armazenamento, e não de acesso.
Nos backends Postgres e LMDB não existe esse atraso: os cliques saem na mesma
transação que o resto.

### O realm do Keycloak é apagado, e pode ficar órfão

Depois que a transação comita, o quark apaga o realm do workspace no Keycloak.
Essa etapa é a última e é best-effort de propósito. Se ela falhar, o workspace
já foi excluído, a API responde `204` do mesmo jeito e o realm fica para trás
como órfão, com um aviso `realm delete failed` no log carregando o slug.

A ordem inversa seria pior: realm apagado com o workspace ainda vivo é um
workspace no qual ninguém consegue entrar. Um realm órfão custa um nome na
lista de realms do Keycloak e mais nada, e não existe rotina automática que
limpe isso, então a limpeza é manual quando o log acusar um.

### Você não é deslogado

Excluir o workspace em que você está não encerra a sua sessão. A linha de
sessão pertence ao workspace e cai junto com ele, então uma nova é emitida
apontando para um workspace do qual você ainda participa, que com certeza
existe porque excluir o último é recusado. A requisição seguinte funciona e o
painel te deixa no workspace que sobrou. Excluir um workspace que não é o atual
não mexe na sua sessão.

Os outros membros não são avisados. Não existe canal de notificação no produto,
então para eles o workspace simplesmente para de aparecer na lista.

## Códigos de status

| Status | Quando |
|---|---|
| `204` | Excluído |
| `401` | Sem cookie de sessão |
| `403` | Membro do workspace, mas não é o `Owner` dele |
| `404` | Instância single-tenant (OSS), ou workspace do qual você não é membro |
| `409` | É o único workspace do qual você participa |
| `503` | O store falhou; nada foi excluído |

Esse endpoint não aceita token de API, só sessão de navegador. Excluir
workspace não é operação de automação, e um token vazado não pode ter esse
poder.
