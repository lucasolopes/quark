# Análise de lacunas: mock de design cloud vs painel real do Quark

Data: 2026-07-14
Fonte do design: Design Composer HTML (landing bilíngue + mock do painel admin).
Painel real conferido: `quark/web/src/` (React 19, Tailwind v4, shadcn/base-ui).

Método: cada item abaixo foi verificado contra o código real (`routes/`, `components/`, `app/Shell.tsx`, `app/router.tsx`, `index.css`, `lib/types.ts`), não por chute. "Presente / parcial / ausente" reflete o que está no `src` hoje.

## Contexto do que já existe (não é lacuna)

Já entregue no painel real e conferido no código: sidebar agrupada (`Shell.tsx`, grupos LINKS/DATA/AUTO/DEV), diálogo de QR (`LinkQrDialog`), stats por link com gráficos (`LinkStats` + `StatsCharts`), CRUD de webhooks com segredo de assinatura e rotação (`Webhooks.tsx`), CRUD de tokens (`Tokens.tsx`), pixels GA4/Meta (`Pixels.tsx`), importação (`Import.tsx`), app-links (`AppLinks.tsx`), regras geo/device dentro do diálogo de criar/editar (`RulesEditor.tsx`), variantes A/B e parâmetros UTM embutidos no diálogo de criação (`CreateLinkDialog.tsx`). Os tokens de marca (fontes Space Grotesk / Hanken / JetBrains Mono, lime `#c6f94e`, utilitário `glow-accent`) já estão em `index.css`. A blocklist foi removida de propósito nesta sessão e não entra como lacuna.

---

## (A) Painel — organização de links

O mock traz um modelo de dados que o painel real não tem: `folders:[{id,name,count}]` e `tags:[{name,count,color}]`. Os links no mock carregam `folder` e `tags`, e a UI de LINKS é reorganizada em torno disso.

### A1. Pastas (folders)

- **O que é:** cada link pertence a uma pasta (`docs`, `mkt`, `social`). No topo da aba LINKS há chips de pasta com nome e contagem (`{{ fc.name }} {{ fc.count }}`), e a tabela agrupa os links por pasta em seções recolhíveis (`linkGroups` com `g.open`, `collapsedFolders`). Ao criar um link, há um seletor de pasta (`folderPickerOpen`, `folderChoices`, `newFolder`) com opção de criar pasta nova (`newFolderInput`).
- **No painel hoje?** Ausente. Zero referência a "folder" em todo o `web/src`. A tabela (`LinkTable`) é uma lista plana.
- **Escopo:** precisa de backend. Coluna/campo `folder_id` no Store (LMDB e Postgres), endpoints CRUD de pastas (`GET/POST/DELETE /admin/folders`), filtro por pasta no `list links`, contagem por pasta, e migração. No front: chips de pasta, agrupamento recolhível na tabela, seletor de pasta no `CreateLinkDialog`/`EditLinkDialog`.
- **Esforço:** L.
- **Valor:** capacidade real de organização quando o volume de links cresce. Diferencia de um CRUD genérico. Alto valor para quem usa de verdade, mas é o item mais caro do bloco A.

### A2. Trilho de filtro por tags com cor e contagem

- **O que é:** um trilho lateral/superior de tags onde cada tag tem um ponto de cor e uma contagem (`tags:[{name,count,color}]`). Na tabela, cada link mostra suas tags como chips coloridos (`l.tagObjs` com `tg.name`). Na criação há um seletor de tags em popover com multi-seleção (`tagPickerOpen`, `tagChoices`, `tc.on`).
- **No painel hoje?** Parcial. Existe filtro por tag, mas é um `<select>` simples com nomes (`Links.tsx` linhas 118-130) alimentado por `useTags()`, cujo tipo é `TagsResponse { tags: string[] }`. Não há cor nem contagem: `Link.tags` é `string[]` e a entrada no `CreateLinkDialog` é um campo de texto livre separado por vírgula (`parseTagsInput`), sem popover nem cor.
- **Escopo:** cor e contagem precisam de backend (metadados de tag: `{name,count,color}` no Store, endpoint que devolve isso em vez de só nomes). O trilho de chips, os chips coloridos na linha e o popover multi-seleção são frontend, mas dependem desses metadados para a cor.
- **Esforço:** M (front) + S/M (backend do metadado de tag).
- **Valor:** o trilho colorido é um forte sinal visual de produto pensado, reduz bastante a cara de "CRUD gerado". A parte de dados (cor por tag) é o que trava.

### A3. Seletor de pasta e popover de tags no criar/editar

- **O que é:** no formulário de criação, além do destino, há o seletor de pasta (A1) e o popover de tags com cor (A2), integrados ao fluxo.
- **No painel hoje?** Parcial. O diálogo existe e já tem destino, alias, TTL, UTM, regras, variantes e tags por texto. Faltam o seletor de pasta e o popover colorido de tags.
- **Escopo:** frontend, encaixando em A1/A2 (depende do backend de pastas e do metadado de cor).
- **Esforço:** S (depois de A1/A2).
- **Valor:** completa o fluxo de organização; sozinho tem pouco valor.

---

## (B) Painel — novos tabs e ferramentas

### B1. UTM Builder (aba dedicada)

- **O que é:** uma ferramenta com URL base, cinco campos (`source`, `medium`, `campaign`, `term`, `content`), preview do link montado em tempo real (`utmUrl`) com botão de copiar, e uma lista de templates aplicáveis (`utmTemplates`: "Social orgânico", "Newsletter", "Paid search") que preenchem os campos com placeholders como `{network}`, `{issue}`.
- **No painel hoje?** Parcial. Os parâmetros UTM existem embutidos no `CreateLinkDialog`, presos a um link sendo criado. Não há a ferramenta autônoma de montar/copiar uma URL UTM avulsa, nem os templates.
- **Escopo:** frontend puro. É cálculo de string no cliente; nenhum backend. Templates podem ser uma constante no front (ou, se quiser persistir, um endpoint simples depois).
- **Esforço:** S.
- **Valor:** ganho de frente rápido e útil. Ferramenta que profissionais de marketing usam solta, sem precisar criar um link. Bom candidato a enviar já.

### B2. Página de Roteamento (aba dedicada)

- **O que é:** uma aba que escolhe um link (`routeLink`, seletor `pickLink`) e mostra, para ele, a lista de regras com prioridade, condição (`geo`/`device`/`os`), operador, destino, reordenação (seta `↓`), remoção e um destino de fallback (`routeDest`). Abaixo, um painel A/B com variantes, peso (`v.split %`), marcação de vencedor (`v.winner`), cliques observados e porcentagem observada (`v.clickPct`), botão de adicionar variante e iniciar teste.
- **No painel hoje?** Parcial. As regras existem via `RulesEditor` dentro do diálogo de criar/editar link, e há variantes A/B. Falta a página autônoma centrada em um link com: reordenação por prioridade, destino de fallback explícito, e o painel A/B com métricas observadas (peso vs. cliques reais e porcentagem).
- **Escopo:** o reagrupamento e a reordenação são frontend sobre o que já existe. As métricas observadas por variante (cliques reais, % observada, vencedor) precisam de backend: agregação de cliques por variante e endpoint de stats de A/B.
- **Esforço:** M (front do reagrupamento) + M (backend de stats por variante).
- **Valor:** dar um lugar dedicado ao roteamento e mostrar resultado do A/B com número real é diferença entre "configurar regras" e "operar um experimento". Valor real de produto.

### B3. Catálogo de Extensões / Integrações

- **O que é:** uma aba de catálogo com 13 integrações em grade, cada uma com monograma colorido, nome, categoria, descrição bilíngue e botão de conectar: Slack, Discord, Telegram (notif), Zapier, Make, n8n, Google Sheets (auto), GA4, Meta CAPI, Tag Manager, TikTok Events, LinkedIn CAPI (analytics), Notion (dev). Tem filtro por categoria (`extCats`), contagem de conectados (`connectedCount`), e um card em destaque de Webhooks ("featured").
- **No painel hoje?** Ausente. Não há aba de extensões nem catálogo. Webhooks e pixels existem como telas isoladas, sem a vitrine.
- **Escopo:** a vitrine em si é frontend (catálogo estático, filtro, estado de conectado). O botão "conectar" de cada integração é que puxa backend: a maioria dessas integrações (GA4, Meta, TikTok, LinkedIn, GTM, Sheets, Notion) são forwarders server-side reais, cada um com seu trabalho. Dá para enviar o catálogo como página de descoberta que aponta os itens já suportados (webhooks, GA4/Meta via pixels) para as telas existentes, e marcar o resto como "em breve".
- **Esforço:** M (vitrine + fiação para o que já existe); L se for implementar cada conector.
- **Valor:** a vitrine dá sensação de plataforma e ancora o roadmap de integrações. Como página de descoberta apontando para webhooks/pixels, é ganho de frente barato e honesto. Implementar todos os conectores é projeto próprio.

---

## (C) Landing page (site de marketing)

O painel real não tem landing: `router.tsx` só tem `/login` e as rotas do painel. A landing inteira do mock está ausente. É um site público bilíngue (PT/EN) com estas seções:

### C1. NAV sticky com blur
- **O que é:** barra fixa no topo com blur de fundo (`position: sticky`, backdrop translúcido), logo "quark v1.0", links (Features, Benchmarks, Arquitetura), troca PT/EN e botão para o painel/deploy.
- **No painel hoje?** Ausente (não há site público).
- **Escopo:** frontend.
- **Esforço:** S.
- **Valor:** primeira impressão pública; o blur e a barra fixa somem a cara de template.

### C2. HERO
- **O que é:** kicker ("Open-source · Rust · AGPL-3.0"), título com destaque lime ("O código é matemática, não uma linha no banco"), subtítulo, dois CTAs, quatro stat cards (`~22M/s`, `18×`, `~1 MB`, `4 rounds`) e um card de terminal (ver D1).
- **No painel hoje?** Ausente.
- **Escopo:** frontend.
- **Esforço:** M.
- **Valor:** núcleo do pitch. Comunica a proposta técnica (código calculado, não armazenado). Alto valor de marca.

### C3. Como funciona (pipeline matemático)
- **O que é:** seção que explica o pipeline `id 40 bits → Feistel 4 rounds ARX chave → base62`, com três pontos (Rápido, Não-enumerável, Leve) e uma ressalva honesta sobre a não-enumerabilidade ser estatística.
- **No painel hoje?** Ausente.
- **Escopo:** frontend.
- **Esforço:** M.
- **Valor:** conteúdo técnico verdadeiro e específico. Reduz muito a cara de IA porque mostra decisão de engenharia real.

### C4. Calibração / avalanche
- **O que é:** tabela de 1 a 12 rounds mostrando avalanche médio e cobertura, com a linha de 4 rounds destacada (`pick: true`) e a nota "ROUNDS = 4 — avalanche 0,5000 exata".
- **No painel hoje?** Ausente.
- **Escopo:** frontend (dados estáticos medidos).
- **Esforço:** S.
- **Valor:** prova concreta do porquê de 4 rounds. Conteúdo que só quem mediu tem.

### C5. Benchmarks
- **O que é:** barras comparando ops/s (quark 22M vs hashids, Feistel+HMAC, sqids) com multiplicadores, nota do harness (`criterion · cargo bench`).
- **No painel hoje?** Ausente.
- **Escopo:** frontend (usa MeterBar, ver D3).
- **Esforço:** S.
- **Valor:** comparação direta e verificável. Forte.

### C6. Capacidade
- **O que é:** grade de quatro números de carga (`3.399 req/s`, `~2 ms`, `~152k req/s`, `0 erros`) com contexto (k6, oha, VPS Alemanha).
- **No painel hoje?** Ausente.
- **Escopo:** frontend (StatCards).
- **Esforço:** S.
- **Valor:** números de produção reais. Credibilidade.

### C7. Features
- **O que é:** seis cards (Núcleo, Analytics, Proteção contra abuso, API de admin, Arquitetura plugável, Operação) com descrições concretas.
- **No painel hoje?** Ausente.
- **Escopo:** frontend.
- **Esforço:** S/M.
- **Valor:** panorama do produto.

### C8. Arquitetura (embutido vs em escala)
- **O que é:** três camadas (Store, Cache, Analytics) mostrando o modo embutido vs. em escala e a env var que troca cada uma (`QUARK_DATABASE_URL`, `QUARK_VALKEY_URL`, `QUARK_CLICKHOUSE_URL`).
- **No painel hoje?** Ausente.
- **Escopo:** frontend.
- **Esforço:** S/M.
- **Valor:** comunica o diferencial de "plugável por env, sem rebuild". Casa com o princípio de escalar como um todo.

### C9. CTA final + footer
- **O que é:** bloco de fechamento ("Suba o seu em um comando") com dois botões e nota de contribuição, mais footer (AGPL-3.0, built in Rust).
- **No painel hoje?** Ausente.
- **Escopo:** frontend.
- **Esforço:** S.
- **Valor:** fechamento e conversão.

Observação geral (C): a landing é um bloco grande mas quase todo frontend, com conteúdo que já existe (é o Quark real). É o maior salto isolado para tirar a cara de "gerado". Merece um documento próprio de estrutura antes de construir, mas não precisa de backend.

---

## (D) Componentes de marca reutilizáveis e micro-detalhes

O diretório `components/brand/` só tem `QuarkMark.tsx`. Os tratamentos de marca do mock não existem como componentes.

### D1. Terminal
- **O que é:** card de terminal com barra de título ("quark — zsh"), três pontos de semáforo e um exemplo de `curl` mostrando POST de criação e GET 302, terminando em `code = feistel_arx(id, key) · not stored`.
- **No painel hoje?** Ausente.
- **Escopo:** frontend (componente reutilizável).
- **Esforço:** S.
- **Valor:** peça de marca central do hero. Muito reaproveitável (docs, README web). Baixo custo, alto retorno visual.

### D2. StatCard
- **O que é:** card com número grande (fonte display, tabular) e rótulo, usado no hero e na capacidade.
- **No painel hoje?** Ausente como componente. A tela de stats usa gráficos (`StatsCharts`), não esse card de número seco.
- **Escopo:** frontend.
- **Esforço:** S.
- **Valor:** reutilizável na landing e possivelmente em resumos do painel. Barato.

### D3. MeterBar
- **O que é:** barra horizontal com rótulo e valor, usada em benchmarks, avalanche e nas quebras por país/device.
- **No painel hoje?** Parcial. `StatsCharts` já desenha barras de distribuição no painel, mas não há um `MeterBar` de marca isolado e reutilizável para a landing.
- **Escopo:** frontend.
- **Esforço:** S.
- **Valor:** unifica a linguagem visual das barras entre site e painel. Barato.

### D4. Tratamentos de Card (hover lift, glow, dot-grid, borda)
- **O que é:** cards com elevação no hover, brilho lime pontual, motivo de grade de pontos (dot-grid) no fundo do hero, e bordas translúcidas finas (`1px solid rgba(255,255,255,.06)`).
- **No painel hoje?** Parcial. O utilitário `glow-accent` existe em `index.css` (usado com parcimônia) e a marca do logo tem `drop-shadow` lime. Faltam hover lift, dot-grid e o padrão de borda da landing.
- **Escopo:** frontend (CSS/utilitários).
- **Esforço:** S.
- **Valor:** micro-detalhes que somados tiram a cara de template. Baratos.

### D5. Rodapé de status do nó na sidebar
- **O que é:** no pé da sidebar do painel, um indicador de nó ("nó q1 · :8080") com o `QUARK_NODE_ID` e a porta.
- **No painel hoje?** Ausente. `Shell.tsx` não tem rodapé na `<aside>`; a sidebar termina na navegação.
- **Escopo:** frontend se for estático/placeholder; pequeno backend se for mostrar o nó e porta reais (endpoint de health/info, algo como `GET /admin/info` com `node_id`).
- **Esforço:** S (front) + S (endpoint info, se real).
- **Valor:** detalhe que reforça a identidade de "binário self-hosted que escala horizontal". Bom sinal, custo baixo. Melhor com dado real.

### D6. Nav sticky com blur (site) e transições
- **O que é:** já coberto em C1; o efeito de blur/backdrop e transições suaves são o tratamento.
- **No painel hoje?** Ausente (só na landing).
- **Escopo:** frontend.
- **Esforço:** S.
- **Valor:** ligado à landing.

---

## Recomendação priorizada

### Enviar agora (frontend puro, sem spec de backend)
Ordem sugerida, do maior retorno por esforço:

1. **UTM Builder (B1)** — ferramenta autônoma, cálculo no cliente, útil de imediato. S.
2. **Componentes de marca D1/D2/D3 (Terminal, StatCard, MeterBar)** — baratos, reutilizáveis, destravam a landing depois. S cada.
3. **Rodapé de status do nó (D5), versão estática** — S, um detalhe que valoriza. Depois pluga no endpoint info.
4. **Micro-detalhes D4 (hover lift, dot-grid, borda)** — S, somados dão o maior salto de "não parece gerado".
5. **Landing page (C1 a C9)** — quase toda frontend, conteúdo já existe (é o Quark real). É o maior ganho isolado de imagem. Vale um documento curto de estrutura primeiro, mas nenhum backend. Construir sobre D1/D2/D3.
6. **Catálogo de Extensões (B3), versão vitrine** — página de descoberta que aponta para webhooks e pixels já existentes e marca o resto como "em breve". M, honesto, sem prometer conectores que não existem.

### Precisam de spec de backend antes
1. **Pastas (A1)** — o item mais caro e mais estrutural. Precisa de `folder_id` no Store (LMDB + Postgres), CRUD de pastas, filtro e contagem no `list`, migração. Escrever spec próprio. L.
2. **Metadado de cor/contagem de tags (A2)** — estender o modelo de tag de `string[]` para `{name,count,color}` no backend; o trilho colorido e o popover só ganham cor com isso. S/M backend, M front.
3. **Métricas observadas de A/B na página de Roteamento (B2)** — agregação de cliques por variante e endpoint de stats. A reorganização visual do roteamento é frontend, mas o número real do experimento é backend. M + M.
4. **Conectores reais das integrações (B3 completo)** — cada forwarder server-side (GA4 já via pixels, Meta idem, mais TikTok/LinkedIn/GTM/Sheets/Notion) é trabalho próprio. Tratar como roadmap, não como uma entrega.

### Ordem geral proposta
Primeiro a leva de frontend puro (1 a 4 acima), porque tira a cara de "gerado" a custo baixo e sem tocar no core. Em paralelo, escrever o spec de Pastas (A1), que é a maior lacuna de capacidade real e destrava A2/A3. Depois a landing (C), que é grande mas isolada e sem risco de backend. Roteamento com métricas (B2) e conectores (B3 completo) entram por último, cada um com seu spec.
