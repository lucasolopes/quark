import { useMe } from "@/lib/queries";

/** Scope names the backend grants (snake_case, matching `crate::auth::Scope`). */
export type ScopeName = "links_read" | "links_write" | "analytics" | "webhooks" | "full";

/**
 * Role/scope-aware gating for the panel UI, read from `/admin/me`'s `scopes`.
 * The backend always enforces these too — this only hides affordances a user
 * cannot use (e.g. a Viewer never sees "Criar link"), so the UI stops offering
 * actions that would 403.
 *
 * `has(scope)` is true when the session carries `full` (covers everything, like
 * `Scope::covers`) or the exact scope. When `scopes` is absent it defaults to
 * granting everything: that is the OSS/break-glass-token path (`useMe` is
 * disabled, so there is no scope list) and the brief pre-load window, where the
 * principal is effectively a full admin and nothing should be hidden.
 */
export function useScopes(): { has: (required: ScopeName) => boolean; scopes: string[] | undefined } {
  const me = useMe();
  const scopes = me.data?.scopes;
  const has = (required: ScopeName) =>
    scopes === undefined || scopes.includes("full") || scopes.includes(required);
  return { has, scopes };
}
