[English](PRIVACY.md) · **Português**

# Privacidade (LUC-44 v1)

Este documento descreve, com precisão, o que o quark captura sobre um
visitante que clica num link curto, o que grava em disco, o que envia pra
fora e o que o quark faz quando o navegador do visitante manda um sinal de
opt-out de privacidade. É escrito pro operador que roda o quark, pra servir
de base factual pro seu próprio aviso de privacidade. Não é aconselhamento
jurídico; a base legal do processamento e o aviso aos usuários finais
continuam sendo responsabilidade sua.

## O que acontece num clique

O redirect (`GET /:code`) é uma resposta simples do servidor: um `302` com
cabeçalho `Location` e `Cache-Control`. Não grava cookie no navegador do
visitante, não roda script client-side e não faz nenhuma requisição a
terceiros a partir do dispositivo dele. Um visitante que clica num link
curto normal não tem nada gravado ou lido do navegador dele.

A analytics de clique do quark é montada inteiramente a partir de dados já
presentes nessa mesma requisição, do lado do servidor:

- `country` e `city`, de cabeçalhos de geo do CDN.
- `referer`, do cabeçalho `Referer`.
- `user_agent`, do cabeçalho `User-Agent`.
- `ip`, do cabeçalho de IP real configurado (ou do socket).
- `fbc`, derivado de um parâmetro de query `fbclid`, quando presente.

É analytics sem cookie: não existe identificador persistente ligando um
clique a outro clique do mesmo visitante. Cada clique é capturado e contado
isoladamente.

## O que vai pro disco

`ip` e `fbc` nunca são persistidos. Existem só em memória, durante o
processamento daquele clique específico, e servem só pra encaminhar um
evento de conversão a um provedor de pixel quando o operador configurou um
(veja abaixo). Ficam excluídos da serialização no nível do código, então não
existe caminho acidental que os grave no buffer de analytics armazenado.

O que é gravado: `user_agent`, `referer`, `country`, `city` e o timestamp do
clique, mais um id de evento por clique. Esse buffer bruto por clique tem um
teto de 1000 eventos por link e é podado de forma circular; não há uma
janela de retenção baseada em tempo nesta versão. Contagens agregadas (por
país, cidade, dispositivo, navegador, host do referer e dia) ficam ao lado
do buffer bruto e não carregam detalhe identificador por clique.

## Global Privacy Control (GPC)

O quark honra o cabeçalho `Sec-GPC` da requisição automaticamente. Não há
configuração pra desligar isso: vem ligado por padrão em todo deploy.

Quando o navegador do visitante manda `Sec-GPC: 1` num clique:

- O redirect acontece normalmente. O visitante chega no destino do mesmo
  jeito que chegaria de outra forma.
- O clique não é gravado na analytics do quark. Nenhum `ClickEvent` é
  registrado pra aquele clique.
- O clique não é encaminhado pra nenhum pixel de conversão configurado (GA4,
  Meta).

As duas supressões vêm do mesmo trecho de código, então honrar o GPC uma vez
cobre captura de analytics e encaminhamento de conversão juntos.

O que o GPC **não** muda:

- O próprio redirect e seu comportamento de `Cache-Control`.
- O contador `max_visits` de um link: uma visita continua contando pro
  limite, e o link continua expirando ao atingir o limite, com ou sem GPC.
  Isso é enforcement de ciclo de vida do link, não rastreamento do
  visitante.
- O webhook `link.clicked`, se o operador estiver inscrito nele. Esse
  webhook é uma notificação first-party pro próprio endpoint do operador
  sobre atividade no link dele, não rastreamento de terceiro do visitante,
  então não é afetado pelo GPC.

O GPC tem respaldo legal real como sinal de opt-out numa lista crescente de
jurisdições. O DNT (`Do Not Track`) não tem esse respaldo hoje e não é lido
pelo quark.

## O único cookie que um visitante pode receber

O único cookie que o quark grava no navegador de um visitante é o
`qk_pw_<code>`, e só depois que esse visitante envia a senha correta de um
link protegido por senha. É `HttpOnly`, `SameSite=Lax`, `Secure` sobre
HTTPS, assinado com HMAC, restrito àquele único link, e expira em 12 horas.
Não carrega nenhum identificador que permita ao quark ou a qualquer outro
reconhecer o mesmo visitante entre links diferentes; existe só pra lembrar
"esse navegador já digitou a senha desse link". É um cookie funcional,
gravado em resposta direta a uma ação do próprio visitante, na mesma
categoria de um cookie de login ou de carrinho de compras.

## Cookies fora do escopo de consentimento do visitante

O painel administrativo grava seus próprios cookies first-party pra sessão
de login do operador (sessão OIDC, estado de login e o cookie de estado do
OAuth do Sheets durante o fluxo de integração). Eles autenticam o operador
usando o painel do quark, não o visitante que clica num link curto, e ficam
inteiramente fora da conversa de consentimento do visitante.

## Encaminhamento de conversão: o único ponto em que dado sai do quark

Se o operador configurar um pixel GA4 ou Meta (página **Pixels**), cada
clique é adicionalmente encaminhado, do lado do servidor e depois que o
redirect já foi concluído, pra esse provedor:

- O **GA4** recebe só o código curto do link, o país, o timestamp e um
  client id sintético por instância. Sem IP, sem User-Agent.
- O **Meta CAPI** recebe adicionalmente o IP bruto do cliente, o User-Agent
  bruto e o `fbc` (tudo em texto puro, já que o Meta faz o hash do IP do
  lado dele), mais um código de país com hash SHA-256.

Esse encaminhamento vem desligado por padrão e só roda quando o operador
adiciona um pixel. É o processamento mais sensível que o quark faz, porque é
o único caminho em que o IP e o User-Agent brutos do visitante saem da
fronteira do quark rumo a um terceiro. Ligar isso, e divulgar (ou obter
consentimento, dependendo da sua jurisdição e do uso posterior do dado), é
responsabilidade do operador. `Sec-GPC: 1` suprime esse encaminhamento
automaticamente, junto com a captura de analytics.

## Self-host e residência de dados

Como você roda o binário do quark e escolhe onde o armazenamento dele vive,
o quark já responde "onde o dado do visitante vive e quem o processa" sem
nenhuma configuração separada: a resposta é onde quer que você tenha
implantado.

## O que esta versão não faz

- Não há retenção configurável baseada em tempo pro buffer bruto por
  clique além do teto circular de 1000 eventos por link. Os agregados não
  são podados.
- Ainda não há endpoint de purge por link ou de erasure por visitante.
- Não há controle fino sobre quais campos o Meta CAPI encaminha; é tudo ou
  nada por pixel.
- Não há banner de consentimento de cookie. Dado o design sem cookie acima,
  um banner não é necessário pra analytics nativa do quark; se você ligar o
  encaminhamento pro Meta ou GA4, avalie por conta própria suas obrigações
  de consentimento pra essa atividade específica.

Esses pontos ficam como trabalho futuro, não entregues nesta versão.
