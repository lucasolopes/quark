import { AlertTriangle, Check, Loader2, Lock, Mail } from "lucide-react";
import type { ReactNode } from "react";
import { useNavigate, useParams } from "react-router-dom";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";
import { ApiError, oidcLoginUrl } from "@/lib/api";
import { useAcceptInvite, useMe } from "@/lib/queries";

/** The API origin, derived from `oidcLoginUrl()` rather than hardcoded. */
const API_ORIGIN = oidcLoginUrl().replace(/\/admin\/login$/, "");

/**
 * Maps an accept-invite failure to its i18n copy. 404/410 both mean the token
 * is unknown or past its expiry (the API doesn't distinguish them for this
 * screen); 403 means the signed-in identity isn't the invited one; 409 means
 * the signed-in identity is already a member of that workspace.
 */
function errorMessage(t: ReturnType<typeof useT>, err: unknown): string {
  if (err instanceof ApiError) {
    if (err.status === 403) return t("accept.errorEmailMismatch");
    if (err.status === 409) return t("accept.errorAlreadyMember");
    if (err.status === 404 || err.status === 410) return t("accept.errorExpired");
    if (err.status === 429) return t("common.rateLimited");
  }
  return t("accept.errorGeneric");
}

/**
 * Shared out-of-Shell frame: the same backdrop as `Login`/`Onboarding` (glow +
 * dot-grid, `-z-10` so it never paints over the centered content regardless of
 * DOM order). Reused across every render branch below so the loading/gate/
 * content states never flash a different background against each other.
 */
function AcceptInviteFrame({ children }: { children: ReactNode }) {
  return (
    <div className="relative min-h-svh overflow-hidden bg-background">
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-hero-glow" />
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-dot-grid" />
      <div className="flex min-h-svh items-center justify-center p-4">{children}</div>
    </div>
  );
}

/** Glyph + display title header, identical to `Login`/`Onboarding`'s. */
function AcceptInviteHeader({ title }: { title: string }) {
  return (
    <div className="mb-[30px] flex flex-col items-center text-center">
      <QuarkMark className="mb-[18px] size-[42px] text-primary glow-glyph" />
      <h1 className="font-heading text-[26px] font-bold tracking-display text-strong">{title}</h1>
    </div>
  );
}

/**
 * Public invite-accept page, rendered OUTSIDE `RequireAuth` — an invitee has
 * no workspace yet, so nesting this under the authed tree would trap them in
 * `WorkspaceGate`/onboarding. Does its own auth check via `useMe` and never
 * auto-accepts: the signed-in identity must be the invited one, so an
 * unauthenticated visitor is sent to sign in first, not accepted on their
 * behalf.
 */
export function AcceptInvite() {
  const { token } = useParams<{ token: string }>();
  const t = useT();
  const navigate = useNavigate();
  const me = useMe();
  const acceptInvite = useAcceptInvite();

  if (me.isLoading) {
    return (
      <AcceptInviteFrame>
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </AcceptInviteFrame>
    );
  }

  if (!me.data?.authenticated) {
    return (
      <AcceptInviteFrame>
        <div className="w-full max-w-[400px] animate-rise">
          <AcceptInviteHeader title={t("accept.title")} />
          <div className="w-full rounded-2xl border border-input bg-card p-6 shadow-modal">
            <div className="mb-5 flex flex-col items-center gap-3 text-center">
              <div data-testid="accept-invite-well" className="flex size-10 items-center justify-center rounded-[9px] bg-secondary">
                <Lock className="size-[18px] text-muted-foreground" aria-hidden="true" />
              </div>
              <p className="text-[14.5px] text-muted-foreground">{t("accept.signInFirst")}</p>
            </div>
            <Button className="w-full" onClick={() => navigate("/login")}>
              {t("login.submit")}
            </Button>
          </div>
        </div>
      </AcceptInviteFrame>
    );
  }

  function handleAccept() {
    if (!token) return;
    acceptInvite.mutate(token, {
      onSuccess: (data) => {
        // Model B: the tenant requires SSO, so accept alone didn't grant
        // membership — hand off to the tenant's login via a full-page
        // navigation (the panel and API are separate origins; a fetch here
        // would just hit CORS, not sign the user in).
        if (data?.status === "login_required" && data.login_url) {
          window.location.assign(`${API_ORIGIN}${data.login_url}`);
          return;
        }
        navigate("/links", { replace: true });
      },
    });
  }

  const alreadyMember = acceptInvite.isError && acceptInvite.error instanceof ApiError && acceptInvite.error.status === 409;

  return (
    <AcceptInviteFrame>
      <div className="w-full max-w-[400px] animate-rise">
        <AcceptInviteHeader title={t("accept.title")} />
        <div className="w-full rounded-2xl border border-input bg-card p-6 shadow-modal">
          <div className="mb-5 flex flex-col items-center gap-3 text-center">
            <div
              data-testid="accept-invite-well"
              className={`flex size-10 items-center justify-center rounded-[9px] border ${
                acceptInvite.isError ? "border-destructive/30 bg-destructive/10" : "border-accent-line bg-accent-wash"
              }`}
            >
              {acceptInvite.isError ? (
                <AlertTriangle className="size-[18px] text-destructive" aria-hidden="true" />
              ) : (
                <Mail className="size-[18px] text-brand-ink" aria-hidden="true" />
              )}
            </div>
            <p className="text-[14.5px] text-muted-foreground">{t("accept.description")}</p>
          </div>
          <div className="flex flex-col gap-3">
            <Button className="w-full" onClick={handleAccept} disabled={acceptInvite.isPending}>
              {acceptInvite.isPending ? (
                <Loader2 className="size-4 animate-spin" aria-hidden="true" />
              ) : (
                <Check className="size-4" aria-hidden="true" />
              )}
              {acceptInvite.isPending ? t("accept.accepting") : t("accept.acceptButton")}
            </Button>
            {acceptInvite.isError && (
              <p role="alert" className="text-center text-sm text-destructive">
                {errorMessage(t, acceptInvite.error)}
              </p>
            )}
            {alreadyMember && (
              <Button variant="outline" className="w-full" onClick={() => navigate("/links", { replace: true })}>
                {t("accept.goToApp")}
              </Button>
            )}
          </div>
        </div>
      </div>
    </AcceptInviteFrame>
  );
}
