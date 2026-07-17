import { Loader2 } from "lucide-react";
import { useNavigate, useParams } from "react-router-dom";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useAcceptInvite, useMe } from "@/lib/queries";

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
      <div className="flex min-h-svh items-center justify-center bg-background">
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </div>
    );
  }

  if (!me.data?.authenticated) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background p-4">
        <Card className="w-full max-w-sm">
          <CardHeader>
            <CardTitle className="font-heading text-2xl font-bold tracking-tight">{t("accept.title")}</CardTitle>
            <CardDescription>{t("accept.signInFirst")}</CardDescription>
          </CardHeader>
          <CardContent>
            <Button className="w-full" onClick={() => navigate("/login")}>
              {t("login.submit")}
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  function handleAccept() {
    if (!token) return;
    acceptInvite.mutate(token, {
      onSuccess: () => navigate("/links", { replace: true }),
    });
  }

  const alreadyMember = acceptInvite.isError && acceptInvite.error instanceof ApiError && acceptInvite.error.status === 409;

  return (
    <div className="flex min-h-svh items-center justify-center bg-background p-4">
      <Card className="w-full max-w-sm">
        <CardHeader>
          <CardTitle className="font-heading text-2xl font-bold tracking-tight">{t("accept.title")}</CardTitle>
          <CardDescription>{t("accept.description")}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-3">
          <Button className="w-full" onClick={handleAccept} disabled={acceptInvite.isPending}>
            {acceptInvite.isPending && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
            {acceptInvite.isPending ? t("accept.accepting") : t("accept.acceptButton")}
          </Button>
          {acceptInvite.isError && (
            <p role="alert" className="text-sm text-destructive">
              {errorMessage(t, acceptInvite.error)}
            </p>
          )}
          {alreadyMember && (
            <Button variant="outline" className="w-full" onClick={() => navigate("/links", { replace: true })}>
              {t("accept.goToApp")}
            </Button>
          )}
        </CardContent>
      </Card>
    </div>
  );
}
