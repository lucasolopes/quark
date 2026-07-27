**English** · [Português](WORKSPACES.PT_BR.md)

# Workspaces

A workspace is a tenant: its own links, click data, analytics, webhooks,
custom domains, invites, API tokens and login realm. Nothing crosses from one
workspace to another, and a user can belong to several of them and switch
between them from the panel.

Workspaces exist only in multi-tenant (cloud) mode. On a single-operator OSS
instance there is one implicit tenant and the `/admin/tenants` routes answer
`404`.

## Roles

Every membership carries a role: `Owner`, `Admin`, `Member` or `Viewer`.
`Owner` and `Admin` currently hold the same permission scopes for everything a
workspace does day to day, and the difference shows up only where the action
destroys the workspace itself.

`Owner` is granted by creating the workspace or by accepting an invite that
names the role, and a login claim can never raise someone to it. `Admin`, on
the other hand, comes from the group in the identity provider's claim, so
whoever administers Keycloak can hand out `Admin`. That is the reason the
deletion below is `Owner` only.

## Creating a workspace

Creating a workspace writes the tenant and the owner membership, seeds the
per-tenant subdomain, and then provisions the login side in Keycloak: a realm,
a client, the role groups and mapper, the owner user, and the set-password
email. Each of those is a separate call to the Keycloak Admin API, so the
request takes noticeably longer than a database insert. The panel says what it
is waiting on instead of only "Creating…".

The workspace row and your membership are committed before the Keycloak work
starts, so reloading the page in the middle is safe: the workspace is already
there in `/admin/me`. If a provisioning call failed, a backfill on the next
boot finishes the realm.

## Deleting a workspace

`DELETE /admin/tenants/:id`, exposed in the panel from the workspace switcher
menu behind a dialog that asks you to type the workspace slug. The confirm
button stays disabled until the text matches.

### Deletion is permanent

There is no trash, no archive and no undo. Everything the workspace owns goes
in a single database transaction:

- links and their aliases, with the redirect rules, variants and passwords
  carried in them
- click counters, click events and the stats the analytics view reads
- link health records and their alert rules
- webhook subscriptions and their delivery history
- API tokens
- conversion forwarding pixels and the hosted well-known documents
- the Google Sheets connection
- custom domains and the per-tenant subdomain
- pending invites
- the workspace's OIDC configuration and its SSO email domains
- active sessions
- the memberships of every member

Your user account is not part of that. Users are global, because the same
person can be a member of other workspaces, so the account survives and only
loses its membership in the deleted workspace.

Because it is one transaction, a database failure leaves the workspace
completely intact and you can try again. There is no half-deleted state.

### Only the Owner can delete

An `Admin` gets `403`, and so does a `Member` or a `Viewer`. The restriction is
deliberate even though `Admin` shares every other permission with `Owner`: the
`Admin` role is derived from a group in the identity provider's claim, so
anyone who controls Keycloak could grant themselves `Admin` and, if the rule
were scope based, destroy the workspace with it. `Owner` cannot be obtained
that way.

Asking to delete a workspace you are not a member of answers `404`, the same
as an id that never existed, so the endpoint cannot be used to find out which
workspaces exist.

### The last workspace cannot be deleted

If the workspace is the only one you belong to, the request is refused with
`409` and nothing is touched. Deleting yourself into having nowhere to land is
not a state the panel has a path out of. Create or join another workspace
first, or leave the workspace where it is.

### The slug becomes available again

Slugs are unique across live workspaces, not reserved forever. Once the
workspace is gone, its row, its subdomain and its Keycloak realm are gone with
it, so the same slug can be used to create a new workspace right away. The new
workspace shares nothing with the old one beyond the name.

### Click data on ClickHouse disappears eventually, not instantly

If your deployment stores click events in ClickHouse, the deletion is issued as
`ALTER TABLE clicks DELETE`, which ClickHouse runs as a mutation: the command
is accepted immediately and executed in the background. The API returns success
at the point the deletion was accepted, so for a window afterwards, usually
seconds to minutes depending on how loaded the cluster is, some of the deleted
workspace's click rows still physically exist in ClickHouse.

The rows are unreachable from the product the moment the workspace is gone.
There is no tenant left to query them under, and no login that could reach
them. What lags is the physical removal on disk, which matters if your
retention or deletion commitments are written in terms of storage rather than
access. On the Postgres and LMDB backends there is no lag: the click rows go
inside the same transaction as everything else.

### On other replicas the links can keep redirecting for up to five minutes

The node that handled the deletion evicts the workspace's hosts from its route
cache as part of the request, so there the links stop resolving as soon as the
API answers `204`. Other replicas only learn about it through the cross-node
invalidation channel, and that channel exists only when `QUARK_VALKEY_URL` is
configured. Without it, each replica keeps serving its own cached route until
the entry expires, which is five minutes.

Inside that window a replica that never got the message still answers `302` for
a link of the deleted workspace. The redirect decodes the short code without
reading the alias table, so the deleted rows are not what stops it: the route
cache is. A single-node deployment never sees this, because the node that
deleted the workspace is the same one serving the redirect.

If you run more than one replica, set `QUARK_VALKEY_URL`. The invalidation is
published the moment the deletion commits and the other replicas drop the route
right away, which closes the window to the time the message takes to arrive.

### The Keycloak realm is deleted, and can be orphaned

After the transaction commits, quark deletes the workspace's Keycloak realm.
That step runs last and best-effort on purpose. If it fails, the workspace is
already deleted, the API still answers `204`, and the realm stays behind as an
orphan with a `realm delete failed` warning in the log carrying the slug.

The inverse order would be worse: a realm deleted while the workspace still
exists is a live workspace nobody can sign in to. An orphaned realm costs a
name in the Keycloak realm list and nothing else, and there is no automatic
reaper for it, so cleaning it up is a manual step when the log shows one.

### You are not logged out

Deleting the workspace you are currently in does not end your session. The
session row belongs to the workspace and goes down with it, so a new one is
issued against a workspace you still belong to, which is guaranteed to exist
because deleting your last workspace is refused. The next request works and the
panel lands you in the remaining workspace. Deleting some other workspace
leaves your current one alone.

Other members are not notified. There is no notification channel in the
product, so from their side the workspace simply stops being listed.

## Status codes

| Status | When |
|---|---|
| `204` | Deleted |
| `401` | No session cookie |
| `403` | Member of the workspace, but not its `Owner` |
| `404` | Single-tenant (OSS) instance, or a workspace you are not a member of |
| `409` | It is the only workspace you belong to |
| `503` | The store failed; nothing was deleted |

An API token is not accepted on this endpoint, only a browser session.
Destroying a workspace is not an automation operation, and a leaked token
should not be able to do it.
