**English** · [Português](LICENSING.PT_BR.md)

# Licensing: Community and Enterprise

quark is open core. The core is free software under the AGPL. A small,
clearly fenced set of directories is proprietary and covers what only matters
when you operate quark as a service for other people.

## The short version

| | Community | Enterprise |
|---|---|---|
| License | AGPL-3.0-only | quark Enterprise Edition License |
| Where | everything except the two paths below | `src/ee/`, `web/src/ee/` |
| Cost | free, no limits | commercial subscription |
| Build | `cargo build` | `cargo build --features ee` |
| Who it is for | one organization running quark for itself | operating quark as a service for others |

Deleting `src/ee/` and `web/src/ee/` leaves a complete, buildable, fully
AGPL-licensed quark. CI proves this on every push, so it cannot quietly stop
being true.

## What is in each edition

**Community has the whole product for one workspace**: the redirect path,
custom short codes, A/B variants, device and geo rules, deep links, link
passwords, expiry, analytics, webhooks, pixels, Google Sheets, Slack, API
tokens, bulk import, broken-link monitoring, the admin panel, and sign-in with
your own identity provider over OIDC.

**Enterprise adds what an operator needs to run quark for other people**:
creating and deleting workspaces, inviting members, per-tenant identity
provider configuration, SSO discovery by email domain, automatic Keycloak realm
provisioning, and multiple custom domains with DNS verification. Billing and
plan limits will land here too.

The line is not "how big is your company". It is whether the accounts you
administer are your own or somebody else's.

## Why the core is AGPL

The AGPL's section 13 says that if you run a modified quark as a network
service, the people using it are entitled to your modifications. That is the
protection the project wants, and it applies to us as well: quark's hosted
service runs the same core that is published here.

Commercial licenses of the core, for using it without the AGPL's copyleft
obligations, are available on request.

## Using the Enterprise code

The `src/ee/` and `web/src/ee/` directories are published as source-available,
not hidden. You may read them, audit them, compile them, and develop against
them. Running them in production requires a valid Enterprise subscription. The
exact terms are in `src/ee/LICENSE` and `web/src/ee/LICENSE`.

Publishing the code is deliberate: nobody should have to trust a black box with
their links, and the parts we charge for should be as inspectable as the parts
we do not.

## Building each edition

```bash
# Community: the default. Contains no Enterprise code at all.
cargo build --release
cd web && npm run build

# Enterprise
cargo build --release --features ee
cd web && VITE_QUARK_EE=1 npm run build
```

The published container image is the Community build.

Tests follow the same split: `cargo test` and `npm run test` cover Community,
`cargo test --features ee` and `npm run test:ee` add the Enterprise surface.

## Environment variables that only do something in Enterprise

`QUARK_MULTI_TENANT`, `QUARK_TENANT_DOMAIN_SUFFIX`, `QUARK_LICENSE_KEY`, and
every `QUARK_KEYCLOAK_*`. A Community build ignores them. See
[`CONFIGURATION.md`](CONFIGURATION.md), where each one is marked.

Plan limits are also Enterprise-only, enforced through the same `--features
ee` gate rather than an environment variable. See
[`PLANS.md`](PLANS.md) for the grid, what each plan unlocks, and how an
operator changes a tenant's plan.

## Contributing

Contributions are welcome in both trees. A pull request that touches `src/ee/`
or `web/src/ee/` is accepted under the Enterprise license of that directory
rather than the AGPL; everything else is AGPL as usual. Either way you keep
ownership of your contribution, and the [CLA](../CLA.md) spells out the grant.

## The reasoning behind the split

The design document, including the benchmarks against n8n, Chatwoot, PostHog,
Cal.com, Dub, Plausible, GitLab, and OpenObserve, is in
[`specs/2026-08-03-luc19-open-core-design.md`](specs/2026-08-03-luc19-open-core-design.md).
The file-by-file inventory that decided where each route and module landed is in
[`research/2026-08-03-luc19-inventario-oss-ee.md`](research/2026-08-03-luc19-inventario-oss-ee.md).
