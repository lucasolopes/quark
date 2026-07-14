# Device-aware redirect (deep linking v1, Option A) — design

**Status:** approved by owner ("pode seguir pelo recomendado" = research Option A). Product decisions recorded below.
**Branch:** `feat/deep-linking` (extends the app-association hosting already there). NOT merged.
**Roadmap:** #20, the redirect half. Builds on the association-file hosting core already on this branch.
**Prereq note:** to be started only after `feat/pixel-forwarding` (#14 user_data) releases the working tree, to avoid corrupting its uncommitted work.

## Problem

The association files (AASA, assetlinks.json) let the OS open the app when it is
installed, and in that case quark never sees the request. When the app is not
installed, or the platform has no app, the tap reaches quark as a normal web
request. quark should be able to send that request to a platform-specific
destination (an App Store / Play Store page, or a platform web page) instead of
one single URL for everyone. That is the device-aware redirect.

## Decisions (owner-approved Option A)

1. **Per-link config**: a link may carry two optional app destinations,
   `app_ios` and `app_android` (both `Option<Url>`). Empty means today's
   behavior (single `url` for everyone).
2. **Platforms**: iOS (iPhone/iPad/iPod) and Android only. Desktop and anything
   else fall through to the normal `url`.
3. **Fallback**: if the click's platform has no configured destination, redirect
   to the link's normal `url`. Never fails.
4. **Hot path**: only links with at least one app destination pay the cost of
   inspecting the User-Agent. The common case (no app destinations) checks one
   `Option::is_none()` and is unchanged. UA classification is a cheap substring
   test, no UA-parsing library.
5. **Deferred deep linking is out of v1** (needs an in-app SDK quark does not
   have; unreliable on iOS post-ATT). Documented as a known limit.
6. **In-app-browser interstitial is out of v1** (reserve).

## Design

### `Record` (src/store/mod.rs)
Two new optional fields:
```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
pub app_ios: Option<String>,
#[serde(default, skip_serializing_if = "Option::is_none")]
pub app_android: Option<String>,
```
Persisted-struct lesson: `#[serde(default)]` + an old-blob deserialization
regression test + Postgres `ALTER TABLE ... ADD COLUMN IF NOT EXISTS` migration
(and `row_to_link` mapping). LMDB stores Record as a JSON blob, so serde(default)
covers it.

### Redirect (src/api.rs)
A pure helper:
```rust
enum Platform { Ios, Android, Other }
fn classify_platform(user_agent: Option<&str>) -> Platform
```
- contains "iPhone" | "iPad" | "iPod" → Ios
- else contains "Android" → Android
- else Other
And a resolver:
```rust
fn app_destination<'a>(rec: &'a Record, ua: Option<&str>) -> Option<&'a str>
```
Returns the platform destination when the record has one for the click's
platform, else `None`. In the redirect handler, AFTER expiry check and BEFORE
building the LOCATION response:
```rust
let dest = if rec.app_ios.is_some() || rec.app_android.is_some() {
    app_destination(&rec, ua).map(str::to_string)
} else {
    None
};
let location = dest.unwrap_or(rec.url);
```
`rec.url` is still moved in the common path (no clone). The `is_some()` guard
means links without app destinations do no UA work.

### Validation (create + patch)
`app_ios` / `app_android`, when present, are validated exactly like the main
`url`: parse as http/https and pass the SSRF guard (`is_blocked_target` /
`is_internal_host`). A destination that fails is a 400, same as the main URL.

### Frontend (web/)
The create/edit link dialog gains an optional "App destinations" section: two
inputs (iOS, Android) with the note that these are used only when the app is not
installed and the click comes from that platform. i18n EN + PT-BR.

### Docs
Extend `docs/DEEP-LINKING.md` + PT_BR with a "Device-aware redirect" section:
what it does, the iOS/Android/fallback table, that it runs only for links that
set an app destination, and the explicit "deferred deep linking (app not
installed) is not handled; that needs an in-app SDK" limit.

## Testing
- Store round-trip with app_ios/app_android (LMDB + gated Postgres); old-blob
  deserialization regression (no app fields → None).
- `classify_platform` unit tests (iPhone, iPad, Android, desktop, empty/None).
- `app_destination` unit tests (ios set + ios UA → ios; android UA + only ios
  set → None → falls back; no fields → None).
- API integration: a link with app_ios set, GET with an iPhone UA → 302 to the
  iOS destination; with a desktop UA → 302 to the normal url; a link with no app
  fields → unchanged. Create/patch with an internal app destination → 400 (SSRF).
- Frontend: the two inputs render and submit; invalid URL feedback.

## Global constraints
Code English; no inline `//`; UI i18n EN+PT; docs EN+PT_BR; SSRF on every
destination (main + app_ios + app_android); hot path pays nothing when no app
destination is set; QUARK_ADMIN_TOKEN unchanged; no merge to main;
avoid-ai-writing on prose; Rust tests with `-j1`.
