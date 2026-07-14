[English](DEPLOY.md) · **Português**

# Deploy do quark no Coolify

O quark é um binário único. Neste repositório há um `Dockerfile` (multi-stage) que
o Coolify detecta automaticamente: não precisa de Nixpacks nem buildpack.

## Passo a passo (Coolify)

1. **Suba o repositório no GitHub** (veja os comandos no README / no fim deste doc).
2. No Coolify: **New Resource → Application → do seu repositório GitHub**.
3. **Build Pack: Dockerfile** (o Coolify detecta o `Dockerfile` na raiz).
4. **Porta exposta: `8080`** (o container escuta em `0.0.0.0:8080`).
5. **Variáveis de ambiente:**
   | var | valor | obrigatório |
   |---|---|---|
   | `QUARK_KEY` | um `u64` aleatório: gere com `openssl rand -hex 8` e converta pra decimal, ou use um número grande. **Configure como _secret_.** | **sim** (sem ela o quark usa uma chave de dev e avisa no log) |
   | `QUARK_ADDR` | `0.0.0.0:8080` | já é o default da imagem |
   | `QUARK_DATA` | `/data` | já é o default da imagem |
6. **Armazenamento persistente:** adicione um **Persistent Storage / Volume** montado em **`/data`**. É onde o LMDB guarda os links; sem isso, os links somem a cada redeploy.
7. **Health check:** caminho **`/health`** (o quark responde `200 ok`). Configure o health check HTTP do Coolify pra esse path na porta 8080.
8. **Deploy.** O Coolify builda a imagem e sobe. O domínio que ele te der já serve os redirects.

## Testando após o deploy

```bash
# criar um link (troque <URL> pelo domínio que o Coolify te deu)
curl -s -XPOST https://<URL>/ -H 'content-type: application/json' \
  -d '{"url":"https://example.com"}'
# -> {"code":"XXXXXXX","url":"https://example.com"}

# seguir o redirect
curl -si https://<URL>/XXXXXXX   # deve responder 302 Location: https://example.com

# health
curl -s https://<URL>/health     # -> ok
```

## Notas de operação

- **Chave por instância:** troque `QUARK_KEY` e todo o espaço de códigos muda. Mantenha-a estável em produção (trocar invalida os códigos já emitidos) e fora do controle de versão.
- **Backup:** basta copiar o volume `/data` (é o banco LMDB inteiro).
- **Escala:** o contador de IDs é single-node (uma instância). Rodar múltiplas réplicas exigiria particionar o espaço de IDs: fica pra fase 2.
