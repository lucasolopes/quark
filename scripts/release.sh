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
set -euo pipefail

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

# Abre a secao da versao nos dois CHANGELOG, logo abaixo de Unreleased. O
# heading tem que casar com `## [x.y.z] - AAAA-MM-DD`, que e o formato exato
# que o guard do release confere e do qual as notas sao extraidas.
add_section() {
  local file="$1" unreleased="$2"
  python - "$file" "$unreleased" "$version" "$today" <<'PY'
import sys
file, unreleased, version, today = sys.argv[1:5]
lines = open(file, encoding="utf-8").read().split("\n")
for i, line in enumerate(lines):
    if line.startswith(unreleased):
        j = i + 1
        while j < len(lines) and not lines[j].startswith("## ["):
            j += 1
        lines[j:j] = [f"## [{version}] - {today}", "", "### Added", "", "### Fixed", "", "### Security", ""]
        open(file, "w", encoding="utf-8").write("\n".join(lines))
        sys.exit(0)
sys.exit(f"erro: nao achei '{unreleased}' em {file}")
PY
}

add_section CHANGELOG.md "## [Unreleased]"
add_section CHANGELOG.PT_BR.md "## [Não lançado]"

cat <<EOF

Versao $version preparada:
  Cargo.toml          -> $version
  CHANGELOG.md        -> secao [$version] - $today
  CHANGELOG.PT_BR.md  -> secao [$version] - $today

Escreva as notas nos dois CHANGELOG agora. Secoes vazias podem ser apagadas.
Quando terminar, de Enter para commitar e taguear, ou Ctrl-C para abortar.
EOF
read -r _

if grep -qE '^### (Added|Fixed|Security)$' CHANGELOG.md && \
   ! sed -n "/^## \[$version\]/,/^## \[/p" CHANGELOG.md | grep -qE '^- '; then
  echo "erro: a secao [$version] do CHANGELOG.md nao tem nenhum item" >&2
  echo "      producao so recebe o que estiver descrito. Escreva as notas." >&2
  exit 1
fi

~/.cargo/bin/cargo.exe metadata --offline --format-version 1 >/dev/null 2>&1 \
  || cargo metadata --offline --format-version 1 >/dev/null

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
