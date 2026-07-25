[English](SECURITY.md) · **Português**

# Política de segurança

## Como reportar uma vulnerabilidade

Não abra issue, discussion ou pull request público para um problema de
segurança.

Reporte de forma privada pelo GitHub:
**https://github.com/lucasolopes/quark/security/advisories/new**

Esse formulário é privado, cria um advisory em rascunho e permite coordenar a
correção e o CVE no mesmo lugar. Não existe e-mail de segurança nem chave PGP: o
formulário de advisory é o único canal.

Inclua, na medida do possível:

- a versão: tag de release, digest da imagem do GHCR ou SHA do commit
  (`quark --version`)
- o formato do deploy: binário único ou Docker, backend de store (LMDB ou
  Postgres), cache (em processo ou Valkey), destino de analytics (embutido ou
  ClickHouse)
- passos de reprodução ou uma prova de conceito, de preferência com `curl`
- o impacto que você acredita que existe

## O que esperar

O quark é mantido por uma pessoa só, no tempo livre dela. Os prazos abaixo são
metas realistas, não um SLA contratual.

| Etapa | Meta |
| --- | --- |
| Primeira resposta humana | 5 dias úteis |
| Decisão de triagem (aceito, não é vulnerabilidade, ou falta informação) | 10 dias úteis |
| Correção publicada para relatos aceitos de severidade alta ou crítica | 30 dias após a triagem |
| Advisory público | junto com a correção, ou 90 dias após o relato, o que vier primeiro |

Se você não receber resposta em 10 dias úteis, abra uma issue pública com o
título "security report awaiting response" e **nenhum detalhe técnico**, que a
gente retoma a conversa.

Seguimos divulgação coordenada. Aguarde 90 dias antes de publicar. Não há
programa de recompensa. Relatos aceitos recebem crédito no advisory, a não ser
que você prefira o contrário.

## Versões suportadas

O quark é pré-1.0. Não existem branches de manutenção e nada é backportado.
Correções entram na `main` e saem na próxima imagem
`ghcr.io/lucasolopes/quark`.

| Versão | Suportada |
| --- | --- |
| `main` e a tag mais recente da imagem no GHCR | sim |
| qualquer tag ou imagem anterior | não, atualize |

## Escopo

Dentro do escopo, mais ou menos em ordem de prioridade:

- previsibilidade ou enumeração de códigos curtos: qualquer coisa que recupere o
  id interno ou o material da chave a partir dos códigos, ou que derrube o
  avalanche medido abaixo do limite calibrado
- bypass de autenticação e autorização do admin: `src/api/guard.rs`, tokens de
  API e escopos em `src/auth.rs`, login OIDC e mapeamento de domínios SSO
- quebra de isolamento entre tenants: ler ou escrever links, domínios ou
  analytics de outro tenant
- bypass de SSRF e open redirect em `src/abuse/` (`is_internal_host`,
  `extract_host`) e na criação de links
- bypass de link protegido por senha, link expirado ou desativado que ainda
  resolve
- forja ou replay de assinatura de webhook (implementação Standard Webhooks)
- XSS, CSRF ou falhas de sessão no painel admin em `web/`
- vazamento de segredos em logs, eventos de analytics ou respostas da API
  (`QUARK_KEY`, `QUARK_ADMIN_TOKEN`, client secrets de OIDC, segredos de
  webhook)
- bypass de rate limit que vire negação de serviço na prática

Fora do escopo:

- ausência de headers de hardening, flags de cookie ou configuração de TLS sem
  exploração demonstrada
- self-XSS, clickjacking em páginas não autenticadas, ou ataques que exijam
  acesso físico ou já privilegiado à máquina
- negação de serviço volumétrica contra uma demo ou instância de terceiros
- saída de scanner automático sem prova de conceito funcionando
- erro de configuração do operador: reusar `QUARK_KEY` entre deploys, subir com
  `QUARK_ADMIN_TOKEN` padrão, expor a API admin na internet sem proxy. Isso está
  documentado em `docs/CONFIGURATION.PT_BR.md`, não é vulnerabilidade.
- vulnerabilidades em Postgres, Valkey, ClickHouse ou outras dependências sem um
  caminho de exploração específico do quark. Reporte no projeto de origem.

Uma observação sobre a `QUARK_KEY`: ela é o segredo por trás da permutação dos
códigos. Quem tiver a chave consegue enumerar todos os códigos daquela
instância. Trate o vazamento dela como comprometimento de todo o namespace de
links e rotacione, o que invalida os códigos existentes.
