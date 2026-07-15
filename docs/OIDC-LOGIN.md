**English** · [Português](OIDC-LOGIN.PT_BR.md)

# Login with your identity provider (OIDC)

By default the panel is opened with the shared `QUARK_ADMIN_TOKEN`. You can also
let operators sign in with their own identity provider (Google, GitHub,
Authelia, Keycloak, or any standard OpenID Connect provider), so accounts are
real, named, and revocable and quark never stores a password.

This is opt-in: OIDC is fully off until you set `QUARK_OIDC_ISSUER`. The admin
token always keeps working as a break-glass, even with OIDC enabled.

## How it works

quark runs the standard OIDC Authorization Code flow with PKCE. It never sees a
password; the provider authenticates the user and returns a signed token that
quark verifies.

```mermaid
flowchart TD
    A[operator clicks Sign in with provider] --> B[GET /admin/login]
    B -->|302 with state + PKCE + nonce| I[Identity Provider]
    I -->|user authenticates| C[GET /admin/callback]
    C --> V[verify state, exchange code, verify id_token]
    V --> M[map the group/claim to scopes]
    M -->|granted| S[create session cookie, open panel]
    M -->|no access| D[403 access denied]
```

Authorization is **default-closed**: a valid provider user gets access only if a
configured group/claim matches. The admin group grants full access; an optional
read-only group grants links-read plus analytics; anyone else is denied.

The session is an opaque, server-side, revocable cookie (`HttpOnly`,
`SameSite=Lax`, `Secure` over HTTPS), valid for 12 hours. Logout revokes it.

## Configuration

Set these on every instance that serves the panel API. OIDC turns on when
`QUARK_OIDC_ISSUER` is present.

| Variable | Purpose |
|---|---|
| `QUARK_OIDC_ISSUER` | Provider issuer URL. Enables OIDC. |
| `QUARK_OIDC_CLIENT_ID` | OAuth client id. |
| `QUARK_OIDC_CLIENT_SECRET` | OAuth client secret. |
| `QUARK_OIDC_REDIRECT_URL` | This instance's callback, `https://<quark-host>/admin/callback`. Register the same value at the provider. |
| `QUARK_OIDC_SCOPES` | Scopes requested (default `openid profile email`). |
| `QUARK_OIDC_ADMIN_CLAIM` | Claim inspected for authorization (default `groups`). |
| `QUARK_OIDC_ADMIN_VALUE` | Value in that claim granting full access (e.g. `quark-admins`). |
| `QUARK_OIDC_READONLY_VALUE` | Optional value granting read-only (links-read + analytics). |

The session cookie is signed with `QUARK_SIGNING_KEY` (the same secret used for
link-password cookies); set it and share it across replicas for a stable
multi-instance deployment.

The panel and the API should be same-origin (or behind one proxy that routes
`/admin/*` to quark) so the session cookie is sent. When they are on different
origins, set `QUARK_CORS_ORIGINS` to the panel origin (quark then allows
credentialed CORS).

## Provider setup

In every provider, register the redirect URI exactly as
`https://<quark-host>/admin/callback`, request the `openid profile email`
scopes, and arrange for a group/role claim so `QUARK_OIDC_ADMIN_CLAIM` /
`QUARK_OIDC_ADMIN_VALUE` can authorize.

### Google

1. Google Cloud Console, APIs and Services, Credentials, create an OAuth 2.0
   Client ID of type "Web application".
2. Add the redirect URI above.
3. `QUARK_OIDC_ISSUER=https://accounts.google.com`, client id/secret from the
   console. Google does not emit group claims by default, so gate on a specific
   account with `QUARK_OIDC_ADMIN_CLAIM=email` and
   `QUARK_OIDC_ADMIN_VALUE=you@example.com` (or use Workspace groups).

### GitHub

GitHub is OAuth2 (not full OIDC). Use it through a broker that speaks OIDC
(Authelia, Keycloak, Dex) pointed at GitHub, or with a GitHub-OIDC bridge. Gate
on the org/team claim the broker exposes.

### Authelia

1. Add quark as an OIDC client in Authelia's configuration with the redirect URI
   above and scopes `openid profile email groups`.
2. `QUARK_OIDC_ISSUER=https://auth.<your-domain>`, the client id/secret you set.
3. `QUARK_OIDC_ADMIN_CLAIM=groups`, `QUARK_OIDC_ADMIN_VALUE=quark-admins` (a
   group you assign to admins in Authelia).

### Keycloak

1. Create a client (Client authentication on, standard flow) in your realm; set
   the redirect URI above.
2. Add a "Group Membership" (or realm-roles) mapper on the `groups` claim.
3. `QUARK_OIDC_ISSUER=https://<keycloak>/realms/<realm>`, client id/secret from
   Credentials, `QUARK_OIDC_ADMIN_CLAIM=groups`,
   `QUARK_OIDC_ADMIN_VALUE=/quark-admins` (Keycloak group paths start with `/`).

## Notes and limits

- Break-glass: `QUARK_ADMIN_TOKEN` always authenticates as full access, so a
  broken IdP never locks you out.
- Stage 2 (a built-in username/password store) is intentionally not built; bring
  your own provider.
- The JWKS is fetched at startup and refreshed automatically when the provider
  rotates signing keys.
- Sessions expire after 12 hours; expired sessions are garbage-collected.
