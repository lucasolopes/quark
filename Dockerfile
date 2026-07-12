# syntax=docker/dockerfile:1

# ---- build ----
# Compila o binário release. A imagem oficial do Rust já traz gcc/cc, que o
# heed precisa para compilar o LMDB (C) e linká-lo estaticamente no binário.
FROM rust:1-bookworm AS build
WORKDIR /app
COPY . .
RUN cargo build --release --bin quark

# ---- runtime ----
# O binário é linkado dinamicamente à glibc (target gnu) e o LMDB vai estático
# dentro dele — então a slim (mesma glibc do bookworm) basta, sem pacote extra.
FROM debian:bookworm-slim
RUN useradd -r -u 10001 quark \
    && mkdir -p /data \
    && chown quark:quark /data
COPY --from=build /app/target/release/quark /usr/local/bin/quark
USER quark
# QUARK_KEY NÃO é definido aqui de propósito — configure como secret no Coolify.
ENV QUARK_ADDR=0.0.0.0:8080 \
    QUARK_DATA=/data
EXPOSE 8080
VOLUME ["/data"]
CMD ["quark"]
