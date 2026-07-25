# syntax=docker/dockerfile:1

# ---- chef: cargo-chef ja instalado na imagem, base para planner e build ----
FROM lukemathwalker/cargo-chef:latest-rust-1-bookworm@sha256:1689f62cfaa6603480356923cb5966544b2dd6ea523e30486bee4f149965d5bc AS chef
WORKDIR /app

# ---- planner: extrai so o grafo de dependencias ----
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---- build ----
# A imagem do cargo-chef ja traz gcc/cc, que o heed precisa para compilar o
# LMDB (C) e linka-lo estaticamente no binario.
FROM chef AS build
COPY --from=planner /app/recipe.json recipe.json
# Esta camada so invalida quando Cargo.lock/Cargo.toml mudam: e o que separa
# um release de 12 minutos de um de 3.
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin quark

# ---- runtime ----
# O binario e linkado dinamicamente a glibc (target gnu) e o LMDB vai estatico
# dentro dele, entao a slim (mesma glibc do bookworm) basta.
#
# ca-certificates NAO entra: `cargo tree -i webpki-roots` confirma que o
# reqwest (via hyper-rustls) e o sqlx embutem as raizes TLS via webpki-roots,
# sem depender do openssl do sistema (`cargo tree -i openssl` nao acha nada).
FROM debian:bookworm-slim@sha256:7b140f374b289a7c2befc338f42ebe6441b7ea838a042bbd5acbfca6ec875818
# gosu: baixar privilegio pra nao-root no entrypoint depois de ajustar o /data.
RUN apt-get update \
    && apt-get install -y --no-install-recommends gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 quark \
    && mkdir -p /data \
    && chown quark:quark /data
COPY --from=build /app/target/release/quark /usr/local/bin/quark
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh
# QUARK_KEY NAO e definido aqui de proposito, configure como secret em runtime.
ENV QUARK_ADDR=0.0.0.0:8080 \
    QUARK_DATA=/data
EXPOSE 8080
VOLUME ["/data"]
# O entrypoint roda como root so pra dar chown no volume (que o orquestrador
# pode montar como root) e entao executa o quark como usuario nao-root via gosu.
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["quark"]
