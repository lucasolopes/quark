# quark

A URL shortener whose short code is a **calibrated, reduced-round ARX permutation** of the internal integer id. The code is not looked up in an index — it is **computed**, in both directions, from a tiny bijective function. That one design choice removes an entire class of problems (collisions) and an entire index (string → id) at once.

## The pitch

Most shorteners pick one of two paths for the code:

- **Reversible encoding** (Hashids, Sqids-style): fast, but not security — codes are partially enumerable. You can scrape `/aaaa`, `/aaab`, …
- **Real cipher** (e.g. Feistly = Feistel + HMAC-SHA256): non-enumerable, but slow — a full cryptographic hash runs on every round.

quark closes that gap with a **Feistel network whose round function is ARX** (add-rotate-xor), not a hash. A Feistel network over an integer domain is a bijection by construction: `decode(encode(id)) == id` for every id, with **zero collision checks needed**, ever. The only open question is *how many rounds* of mixing are needed before the output looks random enough to resist enumeration — and that's not a guess here, it's **measured** (see the avalanche table below). The result is a code generator that is simultaneously non-enumerable *and* orders of magnitude faster than a real-cipher approach, because ARX rounds are cheap integer ops, not hash calls.

Since the code is the permutation of the id, the store never has to index by string. It's keyed by `u64`, straight into an mmap'd database. Millions of links occupy a fraction of what a string-indexed store would need.

## Architecture

```mermaid
flowchart LR
    C[Cliente] -->|POST /| API[api axum]
    C -->|GET /:code| API
    API -->|encode/decode| P[permute + codec]
    API --> CA[cache moka]
    CA -->|miss| ST[(store LMDB)]
    P -.sem I/O.-> API
```

`permute` (the Feistel/ARX bijection) and `codec` (integer ↔ base62) are pure math — no I/O, no locks, off to the side of the request path. The hot path is: decode the code to an id, check the in-memory cache, fall back to a single mmap read on miss.

## Redirect sequence

```mermaid
sequenceDiagram
    participant Cli as Cliente
    participant Api as api
    participant Ca as cache
    participant St as store
    Cli->>Api: GET /:code
    Api->>Api: id = decode(base62⁻¹(code), key)
    Api->>Ca: get(id)
    alt hit
        Ca-->>Api: Record
    else miss
        Ca->>St: get_link(id)
        St-->>Ca: Record | None
        Ca-->>Api: Record | None
    end
    alt achado e não expirado
        Api-->>Cli: 302 Location: url
    else expirado
        Api-->>Cli: 410 Gone
    else não achado
        Api-->>Cli: 404
    end
```

Numeric base62 codes are resolved first, by pure arithmetic (masked, never panics). Only a code that is **not** a valid in-range 7-char base62 string — i.e. wrong length, an invalid character, or a value greater than `MAX_ID` — falls through to a custom alias lookup in the store.

### Aliases

A custom `alias` must not itself be a valid 7-char base62 code in range `0..=MAX_ID`: if it were, it would be indistinguishable from a computed numeric code and would be unreachable (shadowed by the numeric branch above). `create` rejects such aliases with `400 Bad Request` at creation time, before allocating an id, so they never make it into the store.

## How many rounds? Measured, not guessed

The round count for the Feistel/ARX permutation isn't picked by intuition — it's calibrated with an avalanche/SAC (Strict Avalanche Criterion) harness (`cargo run --bin calibrate`), a direct port of the diffusion measurement tooling built for a SHA-256 research lab. The idea behind SAC is simple: **flip one bit of the input id, and on a well-mixed permutation, about half the output bits should flip, unpredictably.** If flipping bit 5 of the id always flips the same 3 output bits, the code is enumerable. If it flips ~50% of the bits, on average, no matter which input bit you flip, the output looks like noise from the outside.

Measured result, sweeping 1 to 12 rounds over 200,000 random samples per round:

```
rounds | avalanche_medio | cobertura(/40)
   1   |     0.1381      |    1
   2   |     0.3622      |   21
   3   |     0.4866      |   40
   4   |     0.5000      |   40   ← ROUNDS escolhido (difusão fecha)
 5..12  |     0.5000      |   40
```

- **avalanche_medio**: average fraction of output bits that flip when one input bit flips (target: 0.5 exactly).
- **cobertura**: the minimum, across all 40 input bits, of how many distinct output bits that single input bit has ever managed to affect. `40/40` means every input bit can influence every output bit — full diffusion, no structural blind spot.

`ROUNDS = 4` is the smallest round count where avalanche hits `0.5000` exactly *and* coverage is full. Round 3 is close (`0.4866`) but not there yet. Rounds 5 through 12 buy nothing — the diffusion has already closed, so quark uses 4 and stops, keeping every round of runtime that isn't needed for the property it's paying for.

## Speed: the trophy number

```
cargo bench --bench permute_bench
```

Measured on this machine (criterion, `benches/permute_bench.rs`):

| op | time/op | ops/sec |
|---|---|---|
| `encode` | ~3.98 ns | ~251,000,000 |
| `decode` | ~3.45 ns | ~290,000,000 |

For comparison, Feistly (Feistel + HMAC-SHA256 per round) does roughly **60,000 ops/sec**. quark's ARX permutation is **~4,000–4,800× faster** — because each round is a handful of adds, rotates and xors, not a cryptographic hash invocation. This is the direct payoff of measuring the minimum round count instead of over-provisioning "for safety": every round saved is real, compounding nanoseconds.

## Running it

```bash
export QUARK_KEY=<a random u64, e.g. from `openssl rand -hex 8`>
export QUARK_DATA=./data        # LMDB directory, created if missing
export QUARK_ADDR=0.0.0.0:8080  # bind address
cargo run --release
```

If `QUARK_KEY` isn't set, quark logs a loud warning and falls back to a hardcoded dev key — fine for local testing, **never for production**: the key is what makes the code space unpredictable per instance.

### curl examples

```bash
# create a short link
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com/some/very/long/path"}'
# => {"code":"01aB2Cd","url":"https://example.com/some/very/long/path"}

# create with a custom alias and a 1-hour TTL
curl -X POST localhost:8080/ -H 'content-type: application/json' \
  -d '{"url": "https://example.com", "alias": "promo", "ttl": 3600}'

# follow it
curl -i localhost:8080/01aB2Cd   # -> 302 Location: https://example.com/...

# health check
curl localhost:8080/health
```

## Threat model — read this before relying on it for secrecy

quark's non-enumerability is a **measured statistical property** (avalanche/SAC over a reduced-round ARX permutation), not a cryptographic guarantee. It resists casual scraping and sequential guessing far better than a raw counter or Hashids-style encoding, and changing `QUARK_KEY` remaps the entire code space. But this is **not AES**, and it is **not** a substitute for real access control if the linked resource itself needs to stay secret — treat codes as "hard to guess by brute force in practice," not "cryptographically secret." Each instance should run with its own random `QUARK_KEY`, kept out of source control.

## More

- Full system design: [`docs/specs/2026-07-12-quark-design.md`](docs/specs/2026-07-12-quark-design.md)
- Deeper walkthrough of every component, data model and the Feistel round internals: [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md)

## License

MIT — see [`LICENSE`](LICENSE).
