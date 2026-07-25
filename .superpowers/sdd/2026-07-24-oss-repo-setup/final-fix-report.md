**English** · Relatorio da leva final de correcoes (review chore/oss-repo-setup)

# Final fix report — chore/oss-repo-setup

Worktree: `C:/Users/L-SALDANHA/pessoal/quark-oss-setup`

## Itens corrigidos

1. **IMPORTANTE 1 — endpoint inexistente no template de bug**
   `.github/ISSUE_TEMPLATE/bug.yml`: placeholder trocado de
   `curl -X POST localhost:8080/api/links -d '{"url":"..."}'` para
   `curl -X POST localhost:8080/ -H 'content-type: application/json' -d '{"url":"..."}'`,
   igual ao quick start do `README.md` e ao campo `url` de `CreateReq` em
   `src/api/links.rs`.

2. **IMPORTANTE 2 — CHANGELOG PT_BR ausente das instrucoes**
   `CONTRIBUTING.md`, `CONTRIBUTING.PT_BR.md` e
   `.github/PULL_REQUEST_TEMPLATE.md` agora pedem explicitamente os DOIS
   arquivos (`CHANGELOG.md` **e** `CHANGELOG.PT_BR.md`) no mesmo PR.

3. **MINOR 3 — interpolacao direta de `${{ }}` no shell**
   `.github/workflows/release.yml`, steps "Le o digest do indice" e "Confere
   que as duas plataformas estao no indice": `${{ needs.guard.outputs.version }}`
   agora passa por `env: VERSION: ...` e o shell le `$VERSION`, no mesmo
   padrao do job `guard`.

4. **MINOR 4 — awk arrastava as link-refs**
   `.github/workflows/release.yml`, extracao das notas do CHANGELOG: acrescentada
   a regra `grab && /^\[.*\]: http/ { exit }`, que para a extracao tambem na
   primeira linha de link-ref. Testado isoladamente contra o `CHANGELOG.md`
   real com `VERSION=0.2.0`: a saida termina em "Private vulnerability
   reporting and a written security policy." e nao inclui mais
   `[Unreleased]: https://...` nem `[0.2.0]: https://...`.

5. **MINOR 5 — comentario errado sobre o flyctl**
   `.github/workflows/ci.yml`: comentario reescrito para explicar o motivo
   real do pin por SHA (o job carrega o token de deploy de producao do Fly;
   pin por SHA e a defesa contra tag movida, o vetor do incidente
   tj-actions/changed-files). Removida a afirmacao falsa de que o Dependabot
   nao consegue atualizar pins com subpath.

6. **MINOR 6 — contradicao sobre binario estatico**
   `README.md` e `README.PT_BR.md`: "static binary" / "binário estático"
   trocado por "single binary" / "binário único". Numero "~1 MB" mantido sem
   alteracao (pre-existente, nao comprovado nem contestado pela tarefa).
   `CONTRIBUTING.md` e `CONTRIBUTING.PT_BR.md` **nao continham** a palavra
   "static"/"estatico" em nenhum lugar (grep case-insensitive confirmou);
   so citam "~1 MB binary" / "binário de ~1 MB" sem qualificador de static,
   entao nao precisaram de edicao.

7. **MINOR 7 — travessao nos comentarios do cla.yml**
   `.github/workflows/cla.yml` linhas 73 e 116: "—" trocado por ":" nas duas
   strings postadas como comentario de PR.

8. **MINOR 8 — CoC apontando para o canal de vulnerabilidade**
   `CODE_OF_CONDUCT.md` e `CODE_OF_CONDUCT.PT_BR.md`: removida a indicacao do
   formulario de security advisory como canal de denuncia de conduta. Agora o
   canal primario e contatar @lucasolopes diretamente no GitHub, com uma nota
   explicita de que o security advisory form e outro canal, reservado a
   vulnerabilidade, com prazos proprios descritos no `SECURITY.md`. O
   paragrafo seguinte (mantenedor unico) ja apontava para
   `https://github.com/contact/report-abuse` como alternativa e ficou como
   estava.

## Verificacao

Todos os comandos do bloco de verificacao da tarefa rodaram limpos (saidas
vazias onde esperado); `python -c "import yaml; ..."` retornou `yaml ok`.
Nenhum arquivo `.rs` foi tocado, entao `cargo fmt`/`clippy` nao se aplicam a
esta leva.
