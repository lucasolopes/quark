# Pastas para organizar links — design

**Branch:** `feat/folders` (off `main`). NÃO mergear até revisar/testar.
**Por quê:** o cloud design organiza links em pastas (Marketing/Docs/Social) com contagem, seletor de pasta na criação e filtro. O painel hoje só tem tags. Um link cabe em **uma** pasta; as **tags continuam livres e múltiplas** por cima (decisão do dono).

## Modelo (lean, espelha tags)

Pasta é um rótulo **singular** no link, análogo a uma tag mas exclusiva, com uma camada de listagem por contagem. Sem tabela de pastas separada nesta versão: uma pasta "existe" enquanto algum link a usa. Criar pasta nova acontece inline ao criar/editar um link (igual ao mock). Pasta vazia, renomear e apagar pasta ficam para uma iteração futura (precisariam de uma entidade de primeira classe).

- `Record.folder: Option<String>` — nome da pasta, `#[serde(default, skip_serializing_if = "Option::is_none")]` para blobs/rows antigos deserializarem como `None`.
- `normalize_folder(raw: Option<String>) -> Option<String>`: trim, corta em `MAX_FOLDER_CHARS` (48), preserva o case para exibição (nomes tipo "Marketing"); string vazia vira `None`.
- Filtro por pasta é **case-insensitive** (compara `to_lowercase`), como o filtro de tag.

## Backend

### `src/store/mod.rs`
- Adiciona `folder: Option<String>` ao `Record` (após `app_android`).
- Adiciona `MAX_FOLDER_CHARS = 48` e `pub fn normalize_folder(...)`.
- `list_links` e `search_links`: novo parâmetro `folder: Option<&str>` (filtra links cuja `folder` bate case-insensitive). Vem **depois** de `tag` na assinatura.
- Novo método de trait: `async fn list_folders(&self) -> Result<Vec<(String, u64)>, StoreError>` — pares (nome, contagem) das pastas distintas, ordenados por nome.

### `src/store/lmdb.rs`
- `list_links`/`search_links`: aplicam o filtro `folder` (além do `tag`) ao varrer.
- `list_folders`: varre os links, conta por `folder` (ignora `None`), devolve ordenado.

### `src/store/postgres.rs`
- Coluna nova `folder TEXT` na tabela de links (idempotente: `ADD COLUMN IF NOT EXISTS`, sem migração destrutiva).
- `list_links`/`search_links`: cláusula `AND lower(folder) = lower($n)` quando `folder` presente.
- `list_folders`: `SELECT folder, count(*) ... WHERE folder IS NOT NULL GROUP BY folder ORDER BY folder`.
- Persistir/ler `folder` no upsert e no SELECT do Record.

### `src/api.rs`
- `CreateReq`/patch: aceitam `folder: Option<String>` (normalizado via `normalize_folder`).
- `LinkRow` (DTO do list): inclui `folder` (`skip_serializing_if = "Option::is_none"`), espelhando `app_ios`.
- `admin_links_list` (`GET /admin/links`): aceita `?folder=` em `ListParams`, repassa para `list_links`/`search_links`.
- Novo `GET /admin/folders` sob `admin_guard(Scope::LinksRead)` → `{"folders":[{"name","count"}]}`.
- Patch de link: `folder` presente no corpo atualiza; para limpar a pasta manda `folder: null` (mesma convenção de `app_ios`).

### `src/store/*` StubStore/mock (webhooks/delivery.rs)
- Implementa `list_folders` (vazio) e o novo parâmetro `folder` nas assinaturas.

## Frontend (`web/`)

### `src/lib/types.ts`
- `Link.folder?: string`; `CreateLinkRequest.folder?: string`; `PatchLinkRequest.folder?: string | null`.
- `interface Folder { name: string; count: number }` e `FoldersResponse { folders: Folder[] }`.

### `src/lib/api.ts` + `queries.ts`
- `listLinks` aceita `folder` no filtro e o manda como `?folder=`.
- `listFolders(): Promise<FoldersResponse>` (`GET /admin/folders`).
- Query `useFolders` (invalida junto com links).

### `src/components/CreateLinkDialog.tsx` + `EditLinkDialog.tsx`
- Campo **Pasta** (opcional): um seletor que lista as pastas existentes (de `useFolders`) e permite digitar uma nova. Simplifica como um `<input list="...">` (datalist) ou um select + campo. Preenche `folder` no request. No Edit, pré-preenche de `link.folder` (regressão testada: espelha o cuidado de `app_ios`).

### `src/routes/Links.tsx` + `src/components/LinkTable.tsx`
- **Filtro de pasta**: ao lado do filtro de tags, um controle que lista as pastas com contagem e filtra a lista (option "Todas as pastas").
- **Coluna/badge de pasta** na tabela (um chip discreto com ícone de pasta), quando o link tem pasta.

### i18n `en.ts` + `pt-BR.ts` (paridade)
- `dialogs.*.folderLabel`, `dialogs.*.folderPh`, `dialogs.*.folderNew`, `links.folderFilterAll`, `linkTable.folder`, etc.

## Comportamento depois
- Criar/editar link permite escolher ou criar uma pasta. A lista filtra por pasta e por tag (combináveis). Cada link mostra sua pasta. `GET /admin/folders` lista pastas com contagem. LMDB single-node inalterado exceto o campo novo; rows/blobs antigos sem `folder` seguem funcionando (`None`).

## Testes
- `store` (LMDB): round-trip de `folder`; `list_folders` conta certo; `list_links(folder=..)` filtra; blob antigo sem `folder` deserializa como `None`.
- `api_it`: create com `folder` → aparece no list e em `/admin/folders`; patch com `folder:null` limpa; filtro `?folder=` narra; ausência de `folder` é omitida no LinkRow (como `app_ios`).
- gated Postgres: mesma bateria (coluna nova, filtro, contagem).
- Frontend Vitest: picker pré-preenche no Edit; filtro chama `listLinks` com `folder`.

## Restrições globais
Código em inglês, sem `//` inline; hot path do redirect intocado; LMDB/single-node inalterado exceto o campo novo; docs EN+PT (API, ARCHITECTURE nota do campo); avoid-ai-writing; Rust `-j1`; Postgres tests gated; sem merge na main até revisar.
