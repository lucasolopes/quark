## What and why

<!--
What changes, and what problem it solves. If there is an issue, link it:
Closes #123
For anything larger than a fix, please open an issue first so we can agree on
direction before you write the code.
-->

## How it was verified

<!--
Beyond CI. What did you actually run or click? For panel changes, add a
before/after screenshot.
-->

## Checklist

<!-- CI already runs fmt, clippy -D warnings, cargo test, cargo-deny, and the
web lint/typecheck/test/build job. The CLA bot handles the CLA. These are the
things automation cannot check. -->

- [ ] Behavior changes are covered by tests (`tests/*_it.rs` for API surface,
      inline `#[cfg(test)]` for units, Vitest for `web/`)
- [ ] Docs updated, **including the `.PT_BR.md` twin**, if behavior, config, or
      the API changed
- [ ] New `QUARK_*` variables documented in `docs/CONFIGURATION.md` and its twin
- [ ] New panel strings added to **both** `web/src/i18n/en.ts` and
      `web/src/i18n/pt-BR.ts`
- [ ] `CHANGELOG.md` **and** `CHANGELOG.PT_BR.md` updated under
      `## [Unreleased]`, or not applicable
- [ ] No new runtime dependency, or the PR explains why it is worth the binary
      size and the "no runtime deps" promise
- [ ] The redirect hot path (`/:code`) stays allocation-light; if it was
      touched, `cargo bench` numbers are in the description

## Breaking changes and migration

<!-- Config, API, storage layout, or panel behavior that an operator has to act
on when upgrading. Write "none" if there are none. -->
