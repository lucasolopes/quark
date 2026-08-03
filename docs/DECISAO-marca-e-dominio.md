# Decisão: marca e domínio (LUC-144)

Levantamento e decisão sobre o uso de `quarkus.com.br` pelo quark. A execução
da troca está na LUC-147. Não é
aconselhamento jurídico: é apuração factual com fontes citadas, feita para
decidir com informação em vez de com sensação.

## 1. A premissa da issue está desatualizada

A LUC-144 foi aberta dizendo que "Quarkus é marca registrada da Red Hat". Os
dois pedaços dessa frase estão errados hoje.

**O dono mudou.** A Red Hat doou a propriedade intelectual do Quarkus, incluindo
a marca e os domínios, para a **Commonhaus Foundation**. A intenção foi anunciada
em 2 de agosto de 2024 e a transferência se concretizou. Quem enforça hoje é uma
fundação com política de marca pública, não uma empresa.

**A marca não é registrada.** Na própria lista de marcas da Commonhaus,
`Hibernate` e `WildFly` aparecem como **®**, com registro em várias jurisdições
(o Hibernate inclusive no Brasil). `Quarkus` aparece como **™**, ou seja, marca
não registrada, de direito comum.

Isso muda bastante o quadro no Brasil, que adota o sistema **atributivo**: o
direito de exclusividade nasce do registro no INPI, não do uso. Sem registro
brasileiro, o caminho de enforcement contra um `.com.br` é mais estreito.

## 2. Mas o risco não é zero

O regulamento do **SACI-Adm** (o procedimento do registro.br para disputa de
domínio) aceita como fundamento, além de marca registrada ou depositada no INPI,
a **marca notoriamente conhecida no Brasil no seu ramo de atividade** (art. 126
da LPI, art. 6º bis da Convenção de Paris). Três fatos empilhados incomodam:

1. Quarkus é notoriamente conhecida entre desenvolvedores Java. Não é uma marca
   obscura.
2. O quark é software para desenvolvedores. O ramo é vizinho, não distante. A
   defesa "somos mercados diferentes" é mais fraca do que parece à primeira
   vista.
3. A política de marca da Commonhaus tem como critério declarado justamente a
   "likelihood of confusion", e a fundação centraliza enforcement.

Contra isso pesa que o SACI-Adm exige do requerente provar **má-fé** no registro
ou no uso. Registrar `quarkus.com.br` para um produto chamado "quark" não é
obviamente má-fé. Mas "não é obviamente má-fé" é uma posição defensável, não uma
posição confortável, e defender custa dinheiro e tempo mesmo quando se ganha.

## 3. A exposição hoje é pequena, e essa é a boa notícia

Varredura no repositório: 22 arquivos citam `quarkus`, e **todos** são
infraestrutura ou documento interno.

| Onde | O quê |
|---|---|
| `fly.toml` | `go.quarkus.com.br`, `backend.quarkus.com.br`, sufixo `quarkus.com.br` |
| `web/.env.production` | `backend.quarkus.com.br` |
| `docs/RUNBOOK-prod-deploy.md` | `app.`, `auth.`, `backend.`, `go.` |
| specs antigas | exemplos de host |

O `README` **não** cita. O painel **não** exibe. O produto se chama "quark" em
todo lugar que o usuário vê. Ou seja: trocar hoje é mexer em configuração, não é
rebranding.

## 4. O custo de trocar só cresce

Para um encurtador, link curto quebrado não tem migração graciosa: ou o domínio
antigo redireciona para sempre, virando custo permanente, ou os links morrem.
Hoje o `quark-prod` ainda é ambiente de teste, sem link de usuário real em
circulação, então o custo de trocar é praticamente zero. Depois do lançamento
ele deixa de ser reversível.

## 5. Um argumento de produto, independente de marca

`go.quarkus.com.br` tem 17 caracteres antes da barra. Para um encurtador isso é
ruim por si só: o concorrente direto usa `dub.sh`, e o Short.io vende domínio
curto como feature paga. O domínio atual já era subótimo antes de qualquer
questão de marca. Os dois argumentos apontam para o mesmo lado.

## 6. Decisão

**Trocar o domínio antes do lançamento**, e trocar por dois domínios em vez de
um. Decidido em 2026-08-03.

| Papel | Host | Por quê |
|---|---|---|
| Painel | `app.quark.sh` | |
| API e redirect | `backend.quark.sh` | `QUARK_ADMIN_HOST` |
| IdP | `auth.quark.sh` | |
| Subdomínio por tenant | `<slug>.quark.sh` | `QUARK_TENANT_DOMAIN_SUFFIX` |
| Link curto compartilhado | `qrk.sh` | `QUARK_PUBLIC_HOST`, 10 caracteres com o código |

Os dois papéis têm requisitos opostos: o host de link curto quer o menor número
de caracteres possível, e o resto quer ser legível. Juntar os dois num domínio
só penaliza o link público, que é o que circula. Dub (`dub.co` mais `dub.sh`) e
Short.io (`short.io` mais `short.gy`) fazem a mesma separação.

Não há legado a preservar: o `quark-prod` é ambiente de teste e não existe link
de usuário em circulação. O `quarkus.com.br` é abandonado, sem período de
redirect.

### Por que `.sh`

Registro aberto, território estável, e é o TLD que o concorrente direto usa. As
alternativas livres tanto em `quark` quanto em `qrk` eram `.gg` (estável, caro),
`.im`, `.st` (barato) e `.ee` (lê como Estônia).

Dois TLD foram descartados por risco de continuidade, que para um encurtador é
risco existencial, já que o contrato com o usuário é que o link funcione para
sempre:

- **`.io`**, apesar de `qrk.io` estar livre. O acordo Reino Unido–Maurício sobre
  os Chagos coloca o código `IO` sob risco de sair da ISO 3166-1, e a ICANN tem
  processo de aposentadoria para ccTLD nessa situação.
- **`.ly`**, pelo precedente de apreensão de domínio na Líbia (caso `vb.ly`,
  2010).

## 7. Disponibilidade consultada

registro.br em 2026-08-03 para `.br`, RDAP para o resto.

| Domínio | Situação |
|---|---|
| `quark.sh`, `qrk.sh`, `qk.sh` | livres |
| `quark.im`, `quark.gg`, `quark.st`, `quark.ee` | livres |
| `qrk.im`, `qrk.gg`, `qrk.st`, `qrk.ee`, `qrk.io`, `qrk.link`, `qk.link` | livres |
| `quarklink.com.br`, `quarkapp.com.br`, `quarkhq.com.br`, `quark.net.br`, `qrk.app.br` | livres |
| `quark.com.br` (expira 2027-01-16), `qrk.com.br` | registrados por terceiros |
| `quark.to`, `qrk.to`, `qk.to`, `quark.dev`, `qrk.dev`, `quark.app`, `qrk.app`, `quark.link` | registrados |

## 8. O que ficou por verificar

Declarado porque a decisão foi tomada sem esses dados, e eles podem mudar o
peso, não a direção.

- **Busca no INPI.** A base (`busca.inpi.gov.br`) recusou conexão nas tentativas
  feitas, então não confirmei se existe depósito ou registro de `QUARKUS` no
  Brasil por alguém, nem por quem. Vale uma busca de anterioridade por agente de
  propriedade industrial, que é barata e é o passo padrão de qualquer jeito.
- **O nome do produto também merece a busca.** `QUARK` é marca da Quark
  Software, Inc. nos EUA, para software de publicação (QuarkXPress), com pedido
  de 2021 (serial 90886976) cobrindo "downloadable computer programs for
  creating, managing, and publishing content", classe 9. É a mesma classe de um
  encurtador. Isso não bloqueia nada sozinho, e "quark" é palavra de vocabulário
  comum, mas existe titular ativo com marca idêntica em software. Esta issue
  trata do domínio; o nome do produto deve entrar na mesma busca de
  anterioridade, antes de qualquer registro de marca ou investimento em
  identidade.

## Fontes

- [Quarkus announces intention to move to Commonhaus Foundation](https://developers.redhat.com/blog/2024/08/02/quarkus-announces-intention-move-commonhaus-foundation), Red Hat Developer, 2024-08-02
- [Quarkus at Commonhaus FAQ](https://quarkus.io/foundation/faq), sobre a doação da marca e dos domínios
- [Commonhaus Foundation Trademarks](https://www.commonhaus.org/trademarks/), onde Quarkus consta como ™ e Hibernate/WildFly como ®
- [Commonhaus Trademark Policy](https://www.commonhaus.org/policies/trademark-policy/) e as [guidelines](https://www.commonhaus.org/policies/trademark-policy/guidelines.html)
- [Regulamento SACI-Adm](https://registro.br/dominio/saci-adm/regulamento/), registro.br
- [Quark Software, Inc. no Justia Trademarks](https://trademarks.justia.com/owners/quark-software-inc-4915702)
