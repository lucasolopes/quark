# Architecture

This document explains how quark works to someone who has never seen the code. It assumes no prior context beyond "it's a URL shortener." For the design rationale and decision log, see [`docs/specs/2026-07-12-quark-design.md`](specs/2026-07-12-quark-design.md); for the pitch and benchmark numbers, see the [README](../README.md).

## Overview

quark is a single Rust binary made of a handful of small, single-purpose modules. Two of them — `permute` and `codec` — do no I/O at all; they're pure functions over integers. Everything else exists to move bytes between the network and the database as cheaply as possible.

```mermaid
flowchart LR
    C[Cliente] -->|POST /| API[api axum]
    C -->|GET /:code| API
    API -->|encode / decode| P[permute + codec]
    API --> CA[cache moka]
    CA -->|miss| ST[(store LMDB/heed)]
    P -.matemática pura, sem I/O.-> API
```

| Module | Responsibility | Depends on |
|---|---|---|
| `permute` | The bijection between id and code: a Feistel network with an ARX round function. `encode(u64) -> u64`, `decode(u64) -> u64`. No state, no I/O. | — (pure core) |
| `codec` | Integer ↔ 7-character base62 string, URL-safe. | — |
| `store` | mmap'd persistence: `id: u64 -> {url, expiry, created}`; a separate `alias -> id` map; a persisted id counter. | `heed` (LMDB bindings) |
| `cache` | A concurrent hot LRU `id -> Record` in front of the store, so a hot redirect never touches LMDB. | `moka` |
| `api` | HTTP surface: `POST /` creates, `GET /:code` redirects, `GET /health`. | `axum` |
| `calibrate` | Offline avalanche/SAC harness that measures diffusion of `permute` across round counts and picks `ROUNDS`. Not part of the running service. | `permute` (a copy of its math, kept dependency-free) |

`permute` and `calibrate` are the differentiator; everything else is standard, swappable engineering (LMDB could become `redb`, moka could become any other cache, axum could become anything that speaks HTTP).

## Create flow

```mermaid
sequenceDiagram
    participant Cli as Cliente
    participant Api as api
    participant St as store
    Cli->>Api: POST / {url, alias?, ttl?}
    Api->>Api: valida url
    Api->>St: next_id() (id atômico persistido)
    Api->>St: put_link(id, {url, expiry})
    Api->>Api: code = base62(encode(id, key))
    Api-->>Cli: {code, url}
    Note over Api,St: sem alias = sem checagem de colisão (bijeção garante)
```

Walking through it: the API validates the URL is `http(s)://`, then asks the store for the next id — a counter persisted in LMDB so it survives restarts. It writes the record keyed by that raw integer id, then computes the public code by running the id through `permute::encode` and base62-encoding the result. Note what's *missing*: there is no "does this code already exist?" check. Because `encode` is a bijection, two different ids can never produce the same code — collision-checking a whole class of bugs out of existence at the type level rather than the runtime level.

Custom aliases are a deliberately separate path: they still allocate a real id and record (so redirect logic doesn't need two code paths), but they route through an `aliases: alias -> id` table that *does* need a uniqueness check, because a human picked the string and two humans can pick the same one. That's the **one** place in the whole system that does a collision check, and it's opt-in.

## Redirect flow

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

quark first tries to parse the path segment as a base62 numeric code and run it through `permute::decode`. If that parse fails (wrong length, invalid character, or the decoded value is out of the valid id range), it falls back to an alias lookup. This means the hot path — numeric codes, which is the overwhelming majority of traffic in a read-heavy shortener — never touches the `aliases` table at all: it's pure arithmetic to get the id, then one cache lookup. Only on a cache miss does it fall through to an LMDB mmap read, which is itself just a page-table lookup in the common case (the OS keeps hot pages resident). Expiry is checked lazily, at read time, against the wall clock — no background sweep is required for correctness, only for eventually reclaiming space.

## The Feistel/ARX permutation

The core trick: quark needs a function `f: [0, 2^N) -> [0, 2^N)` that is a *bijection* — every id maps to exactly one code and vice versa, with no collisions — and that also *looks* random enough that codes aren't guessable from nearby ids. A **Feistel network** gives you the bijection for free, structurally, regardless of what the mixing function inside it does. That's the classical trick behind block ciphers (DES, and format-preserving encryption schemes generally): split the input into two halves `L | R`, and repeatedly do:

```mermaid
flowchart TB
    In["input: L e R"] --> Split[split into two halves]
    Split --> L0[L]
    Split --> R0[R]
    R0 --> F["round_fn(R, key, round) — ARX: add + rotate + xor"]
    L0 --> XOR[L xor F-of-R]
    F --> XOR
    XOR --> NewR["new R = L xor F(R)"]
    R0 --> NewL["new L = R"]
    NewL --> Out["output: novo L e novo R"]
    NewR --> Out
    Out -->|repeat ROUNDS times| Split
```

Why this is *always* invertible, no matter what `round_fn` computes: given the output `(new_L, new_R)`, the previous `R` is just `new_L` (it was passed through untouched), and the previous `L` is `new_R xor round_fn(new_L, ...)` — you recompute the same `round_fn` output and xor it away, since `x xor y xor y == x`. `decode` runs exactly this, round by round, in reverse order. The round function itself never needs to be invertible or even well-behaved for this to hold — that's what makes it safe to make the round function *cheap*.

quark's `round_fn` is ARX (add-rotate-xor): a subkey add, then a small fixed sequence of rotate-xor mixing, masked to the half-width. No hashing, no S-boxes, no cryptographic primitive — just integer ops the CPU does in a cycle or two each. This is exactly the "cost" side of the tradeoff: cheap rounds mean quark can afford to run more of them than a naive scheme would need, if diffusion required it, without paying the multi-hundred-cycle cost of something like HMAC-SHA256 per round.

The remaining question — *how many rounds* — is answered empirically, not assumed. `cargo run --bin calibrate` sweeps `ROUNDS` from 1 to 12 and measures the **avalanche effect**: for every single input bit, flip it, run the permutation, and measure what fraction of the 40 output bits changed. If flipping bit `i` predictably always flips the same handful of output bits, an attacker can reason about the mapping. If it flips ~50% of output bits, on average, no matter which bit you flip, the output is statistically indistinguishable from noise from the outside — that's the Strict Avalanche Criterion (SAC).

```
rounds | avalanche_medio | cobertura(/40)
   1   |     0.1381      |    1
   2   |     0.3622      |   21
   3   |     0.4866      |   40
   4   |     0.5000      |   40   ← ROUNDS escolhido (difusão fecha)
 5..12  |     0.5000      |   40
```

`avalanche_medio` is the average fraction of output bits flipped across all input-bit flips and all sampled inputs; `cobertura` is the worst case, over all 40 input bits, of how many distinct output bits that one bit has ever been observed to influence — it catches structural blind spots that an average alone could hide (e.g. one bit that never reaches the top byte). At round 4, both metrics saturate: avalanche hits exactly `0.5000` and coverage is full `40/40`. Round 3 is close but not there (`0.4866`). Rounds 5–12 measure identically to round 4 — the diffusion has already closed, so there is nothing left to buy by adding more rounds, only latency to lose. `ROUNDS = 4` is fixed as a compile-time constant in `src/permute.rs`, derived directly from this measurement rather than picked by convention or "just to be safe."

## Data model (LMDB)

Three named databases inside one LMDB environment (`heed::Env`), opened once, mmap'd for the process lifetime:

- **`links`**: key = `u64` big-endian (the raw id) → value = JSON-serialized `{ url: String, expiry: Option<u64>, created: u64 }`. This is the only place URL bytes live. Keying by a fixed-width integer instead of the string code means no variable-length string index — B-tree pages pack tighter, and there's no need to ever store or index the base62 code itself, since it's always recomputed from the id.
- **`aliases`**: key = `String` (the human-chosen alias) → value = `u64` (the id it points to). Only touched by custom-alias creates and by redirects whose path segment didn't decode as a valid numeric code.
- **`meta`**: currently one key, `"next_id"` → `u64`, the atomically-incremented id counter, persisted so restarts don't reuse ids.

## Why these choices

- **LMDB via `heed`, not a from-scratch file format or a heavier database**: LMDB is a mmap-backed B-tree — reads are page-cache hits with (in the OS-cached case) essentially no syscall overhead, and there's no separate query engine or network round trip between the process and its data. For a workload that's ~200:1 read:write, an mmap read on the hot path is close to as fast as this gets without inventing a custom on-disk format. `redb` (pure Rust, no FFI) is noted in the spec as a benchmark candidate for later, but LMDB was measured as the faster read path for this use case.
- **`moka` as a cache in front of the store, not just relying on the OS page cache directly**: moka gives a typed, concurrent, capacity-bounded `id -> Record` map so a hit never even reaches the deserialization step — no JSON parse, no LMDB transaction, just a hash lookup returning an already-materialized `Record`. It's a second, cheaper layer on top of what the OS is already doing for the mmap'd pages underneath.
- **Codes computed, never stored**: this is the load-bearing decision the whole design hangs off. Because `encode`/`decode` are a bijection, the code is a pure function of the id and the instance key — there is no `code -> id` table to build, keep consistent, or index. The store's only key type is `u64`. This is also what makes the create path collision-check-free: uniqueness is a mathematical property of the permutation, not something enforced by a runtime lookup.
- **A Feistel network with an ARX round instead of a real cipher**: see the section above — the bijection is free from the network structure; the cost lives entirely in the round function, which was deliberately kept to cheap integer ops and the round count kept to the measured minimum, rather than reusing a cryptographic primitive that would be secure but far slower per operation.
