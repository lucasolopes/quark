# Native Slack / Discord / Telegram notifications — design + plan (roadmap #6)

**Date:** 2026-07-14
**Branch:** `feat/notif-channels` (stacked on `feat/webhooks` / #1; no merge until reviewed). Reviewing/merging this branch also brings #1.
**Effort:** medium. Extends the webhook subscription with a channel `kind` and
per-channel message formatting. No new dependency.

## Goal

"Just send me a message when X happens." Instead of building a raw webhook +
formatter, the operator picks Slack, Discord or Telegram, pastes the channel's
incoming URL, picks events, done. Built on the webhook delivery pipeline (#1).

## Decisions (locked, user delegated)

- **`SubscriptionKind` enum**: `Generic` (default), `Slack`, `Discord`,
  `Telegram`. Add `kind: SubscriptionKind` to `WebhookSubscription` with
  `#[serde(default)]` (persisted struct → old rows default to `Generic`).
- **Message formatting per kind** (plain text, brand-consistent, no emoji):
  - `link.created` → `New short link: {code} -> {url}`
  - `link.updated` → `Short link updated: {code} -> {url}`
  - `link.deleted` → `Short link deleted: {code}`
  - `link.expired` → `Short link expired: {code}`
  - `link.clicked` → `Click on {code} -> {url}` (append ` ({country})` when set)
  Derived by parsing `ev.body` (which already carries `type` + `data.code`/`url`).
- **Delivery per kind** (in `deliver_one`):
  - `Generic`: unchanged. Sign (`whsec_` HMAC) and POST `ev.body` verbatim with
    the three `webhook-*` headers.
  - `Slack`: POST `{"text": <message>}` to `sub.url`.
  - `Discord`: POST `{"content": <message>}` to `sub.url`.
  - `Telegram`: POST `{"text": <message>}` to `sub.url` (the operator pastes
    `https://api.telegram.org/bot<TOKEN>/sendMessage?chat_id=<ID>`; Telegram reads
    `chat_id` from the query and `text` from the body).
  - Channel kinds do NOT sign (the receiver authenticates by the secret URL, not
    our HMAC), so no `webhook-*` headers and `secret` is optional/empty for them.
    SSRF guard (`is_internal_host`) and `redirect(none())` still apply.
- **Create endpoint**: accepts `kind` (default `generic`). Generate an HMAC
  `secret` only for `generic`; for channels leave it empty. Validate the URL is
  http/https + public (same guard). No signature secret is shown for channels.

## Tasks

### Task 1 — backend: kind + per-channel format + delivery branch
Files: `src/webhooks/mod.rs` (`SubscriptionKind`, `kind` field, `format_message`),
`src/webhooks/delivery.rs` (branch by kind), `src/api.rs` (create accepts kind,
secret only for generic; mask/omit secret for channels in the response/list).
- `SubscriptionKind` (serde rename lowercase); `WebhookSubscription.kind`
  `#[serde(default)]`; `format_message(event_type, body_json) -> String`;
  `channel_payload(kind, message) -> Option<String>` (JSON body per kind; `None`
  for Generic).
- `deliver_one` branches: Generic keeps signing; channels POST the channel JSON,
  no signing, `content-type: application/json`.
- Tests: `format_message` per event type; `channel_payload` shapes (`text` for
  slack/telegram, `content` for discord); delivery to a mock server sends the
  Slack-shaped body for a Slack sub; Generic path unchanged (still signed);
  **regression: deserialize a pre-#6 subscription blob without `kind` → `Generic`**.

### Task 2 — frontend: kind selector in the webhook create UI
Files: `web/src/routes/Webhooks.tsx`, `web/src/lib/types.ts`/`api.ts`,
i18n `en.ts`/`pt-BR.ts`, `Webhooks.test.tsx`.
- Create dialog gains a "Type" select (Generic / Slack / Discord / Telegram).
  The URL field label/placeholder/hint adapt per kind (e.g. "Slack incoming
  webhook URL", "Telegram sendMessage URL with chat_id"). The secret notice
  shows only for Generic (channels have no signing secret). The list shows the
  kind as a badge.
- Vitest: selecting Slack + submitting sends `kind: "slack"`; the secret notice
  is hidden for a channel kind.

### Task 3 — docs
Files: `docs/WEBHOOKS.md`/`.PT_BR.md` (a "Notification channels" section: the
three kinds, how to get each URL, that channels are unsigned/URL-is-the-secret,
the message format), ROADMAP (both) marks #6.

## Global constraints

- `kind` is a new field on a PERSISTED struct → `#[serde(default)]` (Generic) +
  a deserialize-old-blob regression test.
- Generic behavior (signing, headers, verbatim body) is UNCHANGED; existing
  webhook tests stay green (safety net).
- Channel kinds: no HMAC signing, secret optional; SSRF guard + `redirect(none())`
  still enforced.
- No new dependency; delivery stays best-effort/fail-open on the same worker.
- All code English; UI via i18n (EN + PT-BR); docs EN + `PT_BR`, no em-dashes.
- Stacked on #1; stay on `feat/notif-channels`; do not merge to main.

## Out of scope

- Rich Slack Block Kit / Discord embeds (plain text first; a formatting upgrade
  can come later).
- Telegram bot-token management UI (the operator pastes a ready sendMessage URL).
- Retry/delivery-durability changes (inherits #1's best-effort model).
