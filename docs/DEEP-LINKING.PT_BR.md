[English](DEEP-LINKING.md) · **Português**

# Deep linking: hospedar os arquivos de associação do app

Pra um link curto abrir um app nativo em vez do navegador, o domínio que serve
o redirect precisa provar que tem permissão de falar por aquele app. iOS e
Android fazem isso do mesmo jeito: buscam um pequeno arquivo JSON no domínio e
conferem contra o app instalado no aparelho. O quark hospeda esses dois
arquivos pra que seus links possam virar Universal Links no iOS e App Links no
Android.

Esta página cobre a hospedagem dos arquivos, que é a parte que o quark entrega
hoje. Abrir o app a cada request (redirect ciente do aparelho) é um follow-up à
parte, veja a nota no fim.

## Os dois arquivos

**iOS: `apple-app-site-association` (AASA).** Um documento JSON que lista os IDs
de app autorizados a tratar links neste domínio e quais caminhos de URL cada app
reivindica. Quando alguém toca num link do seu domínio, o iOS procura esse
arquivo, e se o domínio e o app instalado concordam, o link abre no app em vez
do Safari.

**Android: `assetlinks.json` (Digital Asset Links).** Um documento JSON que diz
qual app Android (nome do pacote mais a impressão digital do certificado de
assinatura) tem permissão de tratar links deste domínio. O Android confere do
mesmo jeito antes de abrir um App Link verificado no app.

Nenhuma das plataformas associa um domínio a um app sem antes conseguir buscar o
arquivo correspondente no domínio. É por isso que hospedar esses arquivos é o
pré-requisito de qualquer comportamento de abrir o app. Sem eles o SO recusa a
associação e todo link simplesmente abre o navegador.

## Como o SO busca os arquivos

O SO pede os arquivos direto, de forma anônima, sobre HTTPS. O quark serve nos
caminhos exatos que cada plataforma procura:

| Arquivo | Caminho que o quark serve |
| --- | --- |
| AASA (iOS) | `/.well-known/apple-app-site-association` |
| AASA (iOS, legado) | `/apple-app-site-association` |
| assetlinks.json (Android) | `/.well-known/assetlinks.json` |

Regras que o SO exige, e que o quark segue:

- **`Content-Type: application/json`.** Os dois arquivos são servidos com esse
  tipo. O AASA não tem extensão `.json`, mas ainda assim é JSON.
- **HTTPS, sem redirect.** O SO busca sobre HTTPS e não segue redirect nesses
  caminhos. O quark serve o arquivo direto no caminho, no mesmo domínio que
  serve seus redirects, então coloque o quark atrás de TLS (um reverse proxy ou
  CDN terminando HTTPS, como no guia de deploy).
- **Sem auth.** A busca é anônima, então essas três rotas GET são públicas.
  Gravar os arquivos é só pra admin (veja abaixo).
- **404 quando não configurado.** Se você não gravou um arquivo, o quark
  responde 404 em vez de um JSON vazio. É o que o SO espera de um domínio que
  não hospeda associação nenhuma.

O caminho de raiz legado `/apple-app-site-association` existe porque algumas
versões antigas do iOS sondam a raiz do domínio antes do caminho `.well-known`.
O quark serve o mesmo documento AASA nos dois.

## Como produzir os arquivos

O quark não gera esses arquivos nem inventa os campos deles. O conteúdo exato
vem do seu time mobile, porque depende do ID do app, do nome do pacote e do
certificado de assinatura. Apple e Google mudam o formato com o tempo (o AASA
passou de `paths` pra `components`, por exemplo), então a referência oficial dos
campos é a doc deles, não esta página:

- Apple, "Supporting associated domains":
  https://developer.apple.com/documentation/xcode/supporting-associated-domains
- Google, "Verify Android App Links":
  https://developer.android.com/training/app-links/verify-android-applinks

O Xcode e o Android Studio emitem esses arquivos como parte do build do app.
Peça ao time mobile o `apple-app-site-association` e o `assetlinks.json` atuais e
cole no quark como estão. O quark valida só que o corpo é JSON válido e está
dentro de um limite de tamanho (64 KiB). Ele não confere os IDs de app nem as
impressões digitais, só o SO e o app podem julgar se a associação está correta.

## Configurar no painel

Abra a página **App Links** no painel de admin. Há dois editores, um pra cada
arquivo:

1. Cole o JSON que o time mobile te passou no editor correspondente.
2. Se o JSON for inválido, o editor sinaliza e o Save fica desabilitado.
   Corrija a colagem até parsear.
3. Clique em **Save** pra gravar e começar a servir o arquivo.
4. Clique em **Clear** pra remover um arquivo gravado (o caminho volta a 404).

Depois do Save, o arquivo fica no ar no caminho well-known na hora. Dá pra
confirmar com uma request, por exemplo
`curl https://seu-dominio/.well-known/assetlinks.json`, e checar o corpo e o
content type `application/json`.

## Ainda não: redirect ciente do aparelho

O redirect ciente do aparelho, abrir de fato o app quando um link é tocado
(detectar iOS ou Android e mandar o aparelho pra uma URI de app ou pra loja, com
fallback web), é um follow-up adiado e ainda não está implementado. Hospedar os
arquivos de associação é o pré-requisito em que esse trabalho se apoia. Ele
precisa de decisões de produto (quais plataformas, esquema de app por link,
comportamento do fallback) e se sobrepõe ao trabalho de regras de redirect,
então fica pra uma rodada posterior, interativa. Hoje o quark hospeda os
arquivos, que é o que permite ao SO associar seu domínio ao app pra começo de
conversa.
