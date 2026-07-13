#!/bin/sh
set -e

# O volume persistente (ex.: Coolify) costuma ser montado com dono root, o que
# impede o usuário não-root de escrever no LMDB. Ajusta o dono do diretório de
# dados e então baixa privilégio pra rodar o quark como usuário não-root.
DATA="${QUARK_DATA:-/data}"
mkdir -p "$DATA"
chown -R quark:quark "$DATA" 2>/dev/null || true

exec gosu quark "$@"
