**English** · [Português](DEPLOY.PT_BR.md)

# Deploying quark on Coolify

quark is a single binary. This repository ships a multi-stage `Dockerfile` that
Coolify auto-detects: no Nixpacks, no buildpack needed.

## Step by step (Coolify)

1. **Push the repository to GitHub** (see the commands in the README / at the end of this doc).
2. In Coolify: **New Resource → Application → from your GitHub repository**.
3. **Build Pack: Dockerfile** (Coolify detects the `Dockerfile` at the repo root).
4. **Exposed port: `8080`** (the container listens on `0.0.0.0:8080`).
5. **Environment variables:**
   | var | value | required |
   |---|---|---|
   | `QUARK_KEY` | a random `u64`: generate one with `openssl rand -hex 8` and convert to decimal, or use any large number. **Set it as a _secret_.** | **yes** (without it quark falls back to a dev key and logs a warning) |
   | `QUARK_ADDR` | `0.0.0.0:8080` | already the image default |
   | `QUARK_DATA` | `/data` | already the image default |
   | `QUARK_STRICT_CLUSTER` | any non-empty value (e.g. `1`) to fail fast unless BOTH `QUARK_DATABASE_URL` and `QUARK_VALKEY_URL` are set. Leave unset for single-node. See [SCALING](SCALING.md). | no (single-node default) |
6. **Persistent storage:** add a **Persistent Storage / Volume** mounted at **`/data`**. This is where LMDB keeps the links; without it, links disappear on every redeploy.
7. **Health check:** path **`/health`** (quark responds `200 ok`). Point Coolify's HTTP health check at that path on port 8080.
8. **Deploy.** Coolify builds the image and brings it up. The domain it gives you already serves the redirects.

## Testing after deploy

```bash
# create a link (replace <URL> with the domain Coolify gave you)
curl -s -XPOST https://<URL>/ -H 'content-type: application/json' \
  -d '{"url":"https://example.com"}'
# -> {"code":"XXXXXXX","url":"https://example.com"}

# follow the redirect
curl -si https://<URL>/XXXXXXX   # should respond 302 Location: https://example.com

# health
curl -s https://<URL>/health     # -> ok
```

## Operating notes

- **One key per instance:** changing `QUARK_KEY` remaps the entire code space. Keep it stable in production (rotating it invalidates every code already issued) and out of version control.
- **Backup:** just copy the `/data` volume (it's the whole LMDB database).
- **Scale:** the id counter is single-node (one instance). Running multiple replicas would require partitioning the id space: that's phase 2.
