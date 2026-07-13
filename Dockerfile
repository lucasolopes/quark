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
# gosu: baixar privilégio pra não-root no entrypoint depois de ajustar o /data.
RUN apt-get update \
    && apt-get install -y --no-install-recommends gosu \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -r -u 10001 quark \
    && mkdir -p /data \
    && chown quark:quark /data
COPY --from=build /app/target/release/quark /usr/local/bin/quark
COPY docker-entrypoint.sh /usr/local/bin/docker-entrypoint.sh
RUN chmod +x /usr/local/bin/docker-entrypoint.sh
# QUARK_KEY NÃO é definido aqui de propósito — configure como secret no Coolify.
ENV QUARK_ADDR=0.0.0.0:8080 \
    QUARK_DATA=/data
EXPOSE 8080
VOLUME ["/data"]
# O entrypoint roda como root só pra dar chown no volume (que o Coolify monta
# como root) e então executa o quark como usuário não-root via gosu.
ENTRYPOINT ["/usr/local/bin/docker-entrypoint.sh"]
CMD ["quark"]
