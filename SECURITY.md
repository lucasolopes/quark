**English** · [Português](SECURITY.PT_BR.md)

# Security Policy

## Reporting a vulnerability

Do not open a public issue, discussion, or pull request for a security problem.

Report it privately through GitHub:
**https://github.com/lucasolopes/quark/security/advisories/new**

That form is private, creates a draft advisory, and lets us coordinate a fix and
a CVE in the same place. There is no security email address and no PGP key: the
advisory form is the only channel.

Please include, as far as you can:

- the version: release tag, GHCR image digest, or commit SHA (`quark --version`)
- the deployment shape: single binary or Docker, store backend (LMDB or
  Postgres), cache (in-process or Valkey), analytics sink (embedded or ClickHouse)
- reproduction steps or a proof of concept, ideally as `curl` calls
- the impact you believe it has

## What to expect

quark is maintained by one person in their own time. These are realistic
targets, not a contractual SLA.

| Step | Target |
| --- | --- |
| First human reply | 5 business days |
| Triage decision (accepted, not a vulnerability, or needs more info) | 10 business days |
| Fix released for accepted high or critical reports | 30 days after triage |
| Public advisory | with the fix, or 90 days after the report, whichever comes first |

If you get no reply within 10 business days, open a public issue titled
"security report awaiting response" with **no technical details** and we will
pick the thread back up.

We follow coordinated disclosure. Please give us 90 days before publishing.
There is no bug bounty. Accepted reports get credit in the advisory unless you
ask otherwise.

## Supported versions

quark is pre-1.0. There are no maintenance branches and nothing is backported.
Fixes land on `main` and ship in the next `ghcr.io/lucasolopes/quark` image.

| Version | Supported |
| --- | --- |
| `main` and the latest GHCR image tag | yes |
| any earlier tag or image | no, upgrade |

## Scope

In scope, roughly ordered by how much we care:

- short code predictability or enumeration: anything that recovers the internal
  id or the key material from codes, or that lowers the measured avalanche below
  the calibrated threshold
- admin authentication and authorization bypass: `src/api/guard.rs`, API tokens
  and scopes in `src/auth.rs`, OIDC login and SSO domain mapping
- tenant isolation breaks: reading or writing another tenant's links, domains,
  or analytics
- SSRF and open redirect bypasses in `src/abuse/` (`is_internal_host`,
  `extract_host`) and in link creation
- password protected link bypass, expired or disabled link still resolving
- webhook signature forgery or replay (Standard Webhooks implementation)
- XSS, CSRF, or session handling flaws in the admin panel under `web/`
- secrets leaking into logs, analytics events, or API responses
  (`QUARK_KEY`, `QUARK_ADMIN_TOKEN`, OIDC client secrets, webhook secrets)
- rate limit bypass that turns into a practical denial of service

Out of scope:

- missing hardening headers, cookie flags, or TLS configuration with no
  demonstrated exploit
- self-XSS, clickjacking on unauthenticated pages, or attacks needing physical
  or already-root access to the host
- volumetric denial of service against a demo or third-party instance
- automated scanner output with no working proof of concept
- operator misconfiguration: reusing `QUARK_KEY` across deployments, shipping a
  default `QUARK_ADMIN_TOKEN`, exposing the admin API to the internet without a
  proxy. Those are documented in `docs/CONFIGURATION.md`, not vulnerabilities.
- vulnerabilities in Postgres, Valkey, ClickHouse, or other dependencies without
  a quark-specific exploit path. Report those upstream.

One note on `QUARK_KEY`: it is the secret behind the code permutation. Anyone
who has it can enumerate every code on that instance. Treat its exposure as a
compromise of the whole link namespace and rotate it, which invalidates existing
codes.
