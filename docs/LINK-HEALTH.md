**English** · [Português](LINK-HEALTH.PT_BR.md)

# Broken-link monitoring

quark can periodically check whether each link's destination still responds and
flag the ones that broke. You shortened a destination months ago, it went down,
and instead of a user complaint you get a notification.

The checker is off by default. It runs only when you set
`QUARK_HEALTH_CHECK_SECS` (the number of seconds between sweeps), so no
background HTTP is ever made unless you ask for it.

## What it does

Every sweep, the checker walks all links and probes each destination with a
`HEAD` request (falling back to `GET` if the server rejects HEAD). A `2xx` or
`3xx` response counts as healthy; a `4xx`, `5xx`, timeout, or connection failure
counts as broken. It does not follow redirects: a `3xx` just means the server is
alive, and not following avoids being bounced toward an internal address.

Destinations on internal or loopback hosts are never probed.

The result is stored per link. In the panel, a small dot on each link shows its
status (green reachable, red broken); a "broken only" filter narrows the list.
The API exposes it as a `health` object on each link row and a `?health=broken`
filter (see [API](API.md)).

When a link changes state, quark emits a webhook: `link.broken` when a healthy
link goes down, `link.recovered` when it comes back. Subscribe to those events
in [Webhooks](WEBHOOKS.md) (or route them to Slack/Discord/Telegram) to be
notified. A destination that is broken the first time it is ever checked fires
`link.broken` once.

```mermaid
flowchart TD
    T[every QUARK_HEALTH_CHECK_SECS] --> L[list all links]
    L --> I{internal host?}
    I -- yes --> S[skip, leave unchecked]
    I -- no --> P[HEAD, fallback GET]
    P --> C{2xx/3xx?}
    C -- yes --> H[healthy]
    C -- no --> B[broken]
    H --> W[write health]
    B --> W
    W --> D{state changed?}
    D -- healthy to broken --> EB[emit link.broken]
    D -- broken to healthy --> ER[emit link.recovered]
    D -- no change --> N[done]
```

## Configuration

| Variable | Effect |
|---|---|
| `QUARK_HEALTH_CHECK_SECS` | Seconds between sweeps. Unset disables the checker. Values below 60 are clamped up to 60. |

In a multi-node deployment the sweep runs only on the designated node (the one
whose `QUARK_NODE_ID` is `0` or unset), so a destination is not probed once per
node. Set `QUARK_HEALTH_CHECK_SECS` on that node.

## Limits

- One probe per sweep per link; a transient failure flips a link to broken and
  the next sweep recovers it (both transitions emit their event).
- The cadence is global; there is no per-link interval or opt-out.
- Health events are best-effort in-memory, like `link.clicked`/`link.expired`.
- The checker is single-node by design; sharding the sweep across nodes is a
  later refinement.
