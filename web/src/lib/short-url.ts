import { useMe } from "@/lib/queries";

/** Hosts a tenant may have for its short links; mirrors the relevant slice of `MeResponse`. */
export interface TenantDomainHost {
  primaryHost?: string | null;
  slug?: string | null;
  suffix?: string | null;
  publicHost?: string | null;
}

/**
 * The public base for short links when no tenant custom host applies: the API
 * host itself (resolves `/:code`), falling back to the origin serving the
 * panel. No trailing slash/protocol, so it composes as `${host}/${code}`.
 */
const PUBLIC_BASE_HOST = (
  (import.meta.env.VITE_API_BASE_URL as string | undefined) || window.location.origin
)
  .replace(/^https?:\/\//, "")
  .replace(/\/+$/, "");

/**
 * Resolves the host (no protocol, no path) used to preview a tenant's short
 * links: verified primary custom domain -> `<slug>.<suffix>` subdomain ->
 * shared public host -> the API/panel origin (OSS, single-tenant). Same
 * precedence as the copy/QR short URL built in `LinkTable`'s `buildShortUrl`;
 * this is the host-only half of it, for the create/edit dialogs' preview.
 */
export function resolveShortHost({ primaryHost, slug, suffix, publicHost }: TenantDomainHost): string {
  if (primaryHost) return primaryHost;
  if (slug && suffix) return `${slug}.${suffix}`;
  if (publicHost) return publicHost;
  return PUBLIC_BASE_HOST;
}

/**
 * The current tenant's short-link host, for the link dialogs' alias prefix
 * and preview. Reads `useMe()` (already fetched/cached elsewhere in the
 * shell) — disabled under break-glass/OSS token auth, in which case this
 * falls back to the panel's own origin, matching `resolveShortHost`.
 */
export function useShortHost(): string {
  const { data: me } = useMe();
  const currentMembership = me?.memberships?.find((m) => m.tenant_id === me.current_tenant);
  return resolveShortHost({
    primaryHost: me?.primary_link_host,
    slug: currentMembership?.slug,
    suffix: me?.tenant_domain_suffix,
    publicHost: me?.public_host,
  });
}
