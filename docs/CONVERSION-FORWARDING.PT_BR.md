[English](CONVERSION-FORWARDING.md) · **Português**

# Encaminhamento de conversão (roadmap #14)

## O que faz

O quark consegue encaminhar um evento de conversão pro GA4 e/ou pro Meta a
cada clique, direto do servidor, sem nenhum tracker rodando no cliente. Não
existe script de pixel, nenhum cookie é gravado no navegador do visitante, e
nenhuma requisição a terceiros sai do dispositivo dele. A página de redirect
nunca conversa com o Google ou com o Meta; quem conversa é o próprio backend
do quark, depois do fato.

Isso é o oposto de como o rastreamento de conversão normalmente funciona. Um
pixel comum carrega no navegador do visitante e liga direto pra plataforma
de anúncio, que é exatamente o que bloqueadores de anúncio, o ITP do Safari
e usuários preocupados com privacidade bloqueiam. Encaminhar o mesmo evento
a partir do servidor contorna tudo isso, ao custo de perder qualquer sinal
client-side (cookies, fingerprint de dispositivo) que um pixel de verdade
capturaria. O que o quark manda é mais grosseiro: um código de link, um país
e um timestamp. Suficiente pra dizer ao provedor "houve um clique nesse
link", não o bastante pra montar um perfil.

## Os dois provedores

- **GA4** (Google Analytics 4), via o **Measurement Protocol**.
- **Meta CAPI** (Meta Conversions API), pra anúncios do Facebook/Instagram.

Os dois são configuração **a nível de instância** nesta etapa: uma ou mais
configurações de pixel vivem sob a conta do operador, e todo clique é
encaminhado pra cada uma que estiver ativa. Ainda não há mira por link (veja
a nota de follow-up no final).

### Como conseguir o API secret do Measurement Protocol (GA4)

1. No GA4, vá em **Admin > Data Streams** e escolha o stream da sua
   propriedade.
2. Dentro desse stream, abra **Measurement Protocol API secrets**.
3. Crie um secret novo. Copie ele, junto com o **Measurement ID** do stream
   (o valor `G-XXXXXXXXXX` mostrado no topo da página do stream).
4. Informe os dois na página Pixels do quark: Measurement ID + API secret.

### Como conseguir o access token da Conversions API (Meta)

1. No **Events Manager**, selecione (ou crie) o pixel pro qual você quer
   encaminhar. Anote o **Pixel ID** dele (um id numérico, mostrado na página
   de visão geral do pixel).
2. Nas configurações desse pixel, vá em **Conversions API** e gere um
   **access token** (ou use um token de System User em Business Settings se
   quiser um que não expire junto com uma troca de conta pessoal).
3. Informe os dois na página Pixels do quark: Pixel ID + access token.

## Postura de privacidade

Só os campos que o quark já captura pra funcionalidade de analytics de
clique são encaminhados:

- o código curto do link (não o id interno),
- o país do clique (já derivado no servidor, por exemplo a partir de um
  header de geo do CDN),
- o timestamp do evento.

**Não é enviado**: o endereço IP do visitante, o User-Agent bruto dele, ou
qualquer outro identificador client-side. O GA4 recebe um `client_id`
sintético (gerado por instância do quark, não por visitante) em vez de um id
real de usuário; o Meta não recebe nenhuma etapa de hashing de dado do
usuário nesta versão, então trate o que chega em qualquer um dos dois
provedores como um **ping de conversão anônimo**, não um evento de usuário
atribuível. Advanced matching (email/telefone com hash) fica explicitamente
fora de escopo por enquanto.

## Assíncrono, fail-open, nunca no caminho quente

O encaminhamento roda a partir do **worker de analytics** que já existe no
quark, o mesmo caminho em background que já grava os eventos de clique no
sink de analytics. Ele **não** roda junto com o redirect: um clique recebe a
resposta 302 imediatamente, independente de GA4 ou Meta estarem
configurados, alcançáveis ou lentos. O worker agrupa os cliques em lote e
encaminha cada lote pra cada configuração de pixel ativa, depois do fato.

Isso também é **fail-open**: se um provedor está fora do ar, limitando taxa
do quark, ou retorna erro, essa falha é logada e descartada. Ela nunca afeta
o redirect, nunca bloqueia o próximo lote do worker, e nunca aparece pro
usuário final. Não existe fila de retry; um lote que falha ao encaminhar não
é tentado de novo. Se um provedor fica fora do ar por um período longo, as
conversões dessa janela simplesmente se perdem, não são recuperadas depois.
É uma escolha deliberada de simplicidade nesta etapa; veja a nota de
follow-up abaixo.

## Sem superfície de SSRF

Os hosts dos provedores (`https://www.google-analytics.com` pro GA4,
`https://graph.facebook.com` pro Meta) são fixos no código. O operador
fornece credenciais (Measurement ID/API secret, Pixel ID/access token), não
URLs, pela página Pixels. Não existe nenhum campo em lugar nenhum que deixe
um operador (ou um atacante que comprometa o painel) apontar as requisições
de saída do quark pra um host arbitrário. O host base só é injetável em
código de teste, nunca pela API ou pela UI.

## Gerenciando pixels

A página **Pixels** no painel web (`/pixels`) lista os pixels configurados,
deixa adicionar um (escolha um provedor, depois preencha os dois campos de
credencial daquele provedor) e remover um. Segredos (`api_secret`,
`access_token`) ficam mascarados como `••••` depois de salvos; os
identificadores (`measurement_id`, `pixel_id`) aparecem em claro já que não
são segredos por si só. Tudo isso fica atrás do mesmo `x-admin-token` usado
pro resto do painel; não existe uma permissão separada pra pixels.

## Follow-ups (fora desta etapa)

- Provedores adicionais (GTM, TikTok, LinkedIn) depois que esse padrão se
  provar.
- Mira por link nos pixels (hoje é todos-os-pixels-ativos, em todo clique).
- Advanced matching / hashing de dado de usuário.
- Retry ou durabilidade além do best-effort atual do worker.
