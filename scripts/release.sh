#!/usr/bin/env bash
# Corta uma release. Producao so recebe deploy por tag, entao este script existe
# para que taguear custe um comando e nao vire o motivo de voce parar de taguear.
#
#   scripts/release.sh 0.2.1
#
# O que ele faz: confere que a arvore esta limpa e na main atualizada, sobe a
# versao no Cargo.toml, abre a secao da versao nos dois CHANGELOG, deixa voce
# editar, e so entao commita e cria a tag. O push fica por sua conta, de
# proposito: e a ultima chance de desistir antes de disparar o deploy.
# O -E (errtrace) e obrigatorio: sem ele o trap ERR nao e herdado pelas
# funcoes, e um add_section que falhasse deixaria o Cargo.toml ja reescrito.
set -Eeuo pipefail

cd "$(dirname "$0")/.."

version="${1:-}"
if [ -z "$version" ]; then
  echo "uso: scripts/release.sh <versao>   (ex.: 0.2.1, ou 0.3.0-rc.1)" >&2
  exit 1
fi

case "$version" in
  v*) echo "erro: passe a versao sem o 'v' (ex.: 0.2.1)" >&2; exit 1 ;;
esac

if ! printf '%s' "$version" | grep -qE '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
  echo "erro: '$version' nao parece semver" >&2
  exit 1
fi

if [ -n "$(git status --porcelain)" ]; then
  echo "erro: a arvore tem mudancas nao commitadas" >&2
  exit 1
fi

branch="$(git rev-parse --abbrev-ref HEAD)"
if [ "$branch" != "main" ]; then
  echo "erro: releases saem da main, voce esta em '$branch'" >&2
  exit 1
fi

git fetch origin main --quiet
if [ "$(git rev-parse HEAD)" != "$(git rev-parse origin/main)" ]; then
  echo "erro: sua main esta diferente de origin/main. Faca pull antes." >&2
  exit 1
fi

if git rev-parse "v$version" >/dev/null 2>&1; then
  echo "erro: a tag v$version ja existe" >&2
  exit 1
fi

today="$(date +%Y-%m-%d)"

# A partir daqui o script reescreve arquivos. Se qualquer coisa falhar antes do
# commit (o PT_BR sem o heading esperado, o cargo recusando, um hook barrando),
# desfaz tudo em vez de deixar a arvore meio editada e o usuario descobrindo
# pelo git status depois. So e desarmado imediatamente antes do commit.
rollback() {
  echo "" >&2
  echo "abortado. Revertendo Cargo.toml e os CHANGELOG." >&2
  git checkout -- Cargo.toml CHANGELOG.md CHANGELOG.PT_BR.md 2>/dev/null || true
}
trap rollback ERR INT

# Cargo.toml: so a linha `version` de dentro do bloco [package], nunca a de
# alguma dependencia.
python - "$version" <<'PY'
import re, sys
version = sys.argv[1]
src = open("Cargo.toml", encoding="utf-8").read()
def bump(m):
    return re.sub(r'(?m)^version = ".*"$', f'version = "{version}"', m.group(0), count=1)
new = re.sub(r'(?ms)^\[package\].*?(?=^\[)', bump, src, count=1)
if new == src:
    sys.exit("erro: nao consegui subir a versao no Cargo.toml")
open("Cargo.toml", "w", encoding="utf-8").write(new)
PY

# MOVE o conteudo acumulado em Unreleased para a secao da nova versao. Isso e o
# ponto inteiro do fluxo: o CONTRIBUTING manda o contribuidor escrever em
# Unreleased, e as notas da release saem de `## [x.y.z]`. Se o script apenas
# abrisse uma secao vazia, tudo que foi escrito em Unreleased ficaria orfao ali
# para sempre e nunca sairia em release nenhuma.
#
# O heading gerado tem que casar com `## [x.y.z] - AAAA-MM-DD`, que e o formato
# exato que o guard do release confere e do qual as notas sao extraidas.
add_section() {
  local file="$1" unreleased="$2"
  python - "$file" "$unreleased" "$version" "$today" <<'PY'
import sys
file, unreleased, version, today = sys.argv[1:5]
lines = open(file, encoding="utf-8").read().split("\n")

start = next((i for i, l in enumerate(lines) if l.startswith(unreleased)), None)
if start is None:
    sys.exit(f"erro: nao achei '{unreleased}' em {file}")

# Fim do bloco Unreleased: o proximo heading de versao, ou o rodape de links.
end = start + 1
while end < len(lines) and not lines[end].startswith("## [") \
        and not lines[end].startswith("[Unreleased]:") \
        and not lines[end].startswith("[Não lançado]:"):
    end += 1

body = [l for l in lines[start + 1:end]]
while body and not body[0].strip():
    body.pop(0)
while body and not body[-1].strip():
    body.pop()

if not body:
    sys.exit(f"erro: '{unreleased}' esta vazio em {file}. Nao ha o que lancar.")

lines[start + 1:end] = ["", f"## [{version}] - {today}", ""] + body + [""]
open(file, "w", encoding="utf-8").write("\n".join(lines))
PY
}

add_section CHANGELOG.md "## [Unreleased]"
add_section CHANGELOG.PT_BR.md "## [Não lançado]"

# Rodape de links. Sem isto o `## [x.y.z]` vira link quebrado no GitHub e o
# [Unreleased] continua comparando contra a versao anterior.
bump_links() {
  python - "$1" "$2" "$version" <<'PY'
import re, sys
file, unreleased_key, version = sys.argv[1:4]
s = open(file, encoding="utf-8").read()
m = re.search(r'(?m)^\[' + re.escape(unreleased_key) + r'\]: (\S+?)/compare/v(\S+?)\.\.\.HEAD$', s)
if not m:
    sys.exit(0)  # rodape em outro formato: nao mexe
base, prev = m.group(1), m.group(2)
s = s.replace(m.group(0),
    f"[{unreleased_key}]: {base}/compare/v{version}...HEAD\n"
    f"[{version}]: {base}/compare/v{prev}...v{version}")
open(file, "w", encoding="utf-8").write(s)
PY
}
bump_links CHANGELOG.md "Unreleased"
bump_links CHANGELOG.PT_BR.md "Não lançado"

cat <<EOF

Versao $version preparada:
  Cargo.toml          -> $version
  CHANGELOG.md        -> secao [$version] - $today, com o conteudo de Unreleased
  CHANGELOG.PT_BR.md  -> secao [$version] - $today, com o conteudo de Unreleased

Revise as notas nos dois CHANGELOG agora. O que estiver ali e exatamente o que
vai sair nas notas da release e no anuncio.
Quando terminar, de Enter para commitar e taguear, ou Ctrl-C para abortar.
EOF
read -r _

if ! sed -n "/^## \[$(printf '%s' "$version" | sed 's/\./\\./g')\]/,/^## \[/p" CHANGELOG.md \
      | grep -qE '^[-*] '; then
  echo "erro: a secao [$version] do CHANGELOG.md nao tem nenhum item" >&2
  echo "      producao so recebe o que estiver descrito. Escreva as notas." >&2
  exit 1
fi

if command -v cargo >/dev/null 2>&1; then
  cargo metadata --offline --format-version 1 >/dev/null
elif [ -x "$HOME/.cargo/bin/cargo.exe" ]; then
  "$HOME/.cargo/bin/cargo.exe" metadata --offline --format-version 1 >/dev/null
else
  echo "aviso: cargo nao encontrado, pulando a validacao do Cargo.toml" >&2
fi

trap - ERR
git add Cargo.toml Cargo.lock CHANGELOG.md CHANGELOG.PT_BR.md
git commit -m "chore: release $version"
git tag -a "v$version" -m "quark v$version"

cat <<EOF

Pronto. Nada foi enviado ainda.

Para disparar a release e o deploy em producao:
  git push origin main && git push origin v$version

Para desistir:
  git tag -d v$version && git reset --hard HEAD~1
EOF
