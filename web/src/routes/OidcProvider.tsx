import { AlertTriangle, RotateCw, ShieldCheck } from "lucide-react";
import { useState, type FormEvent, type ReactNode } from "react";
import { toast } from "sonner";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useDeleteOidcConfig, useMe, useOidcConfig, usePutOidcConfig } from "@/lib/queries";
import type { OidcConfigView } from "@/lib/types";

/**
 * Admin UI for the tenant's own OIDC identity provider (LUC-83). The backend
 * CRUD (`/admin/oidc-config`) already existed; this is the panel form for it.
 * Cloud-only (OSS never has `memberships`) and Owner/Admin-only (the nav item
 * is gated on the `full` scope). In the managed Keycloak deployment the config
 * is auto-provisioned, so this mostly matters for bring-your-own external IdP.
 */
export function OidcProvider() {
  const t = useT();
  const me = useMe();
  const cloud = me.data?.memberships !== undefined;
  const query = useOidcConfig(cloud);

  if (!cloud) return null;

  if (query.isPending) {
    return (
      <div className="flex flex-col gap-2" aria-hidden="true">
        <Skeleton className="h-10 w-full" />
        <Skeleton className="h-40 w-full" />
      </div>
    );
  }

  const notConfigured = query.error instanceof ApiError && query.error.status === 404;

  if (query.isError && !notConfigured) {
    return (
      <div className="flex flex-col gap-4">
        <Header />
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <p className="font-medium">{t("ssoProvider.loadError")}</p>
            <Button variant="outline" onClick={() => query.refetch()}>
              <RotateCw className="size-4" />
              {t("common.retry")}
            </Button>
          </CardContent>
        </Card>
      </div>
    );
  }

  return <OidcProviderForm config={query.data ?? null} />;
}

function Header() {
  const t = useT();
  return (
    <div>
      <h1 className="font-heading text-2xl font-semibold">{t("ssoProvider.title")}</h1>
      <p className="mt-1 text-sm text-muted-foreground">{t("ssoProvider.subtitle")}</p>
    </div>
  );
}

interface FieldState {
  issuer: string;
  clientId: string;
  clientSecret: string;
  scopes: string;
  adminClaim: string;
  adminValue: string;
  memberValue: string;
  readonlyValue: string;
  requiredValue: string;
  postLoginUrl: string;
}

function initialState(config: OidcConfigView | null): FieldState {
  return {
    issuer: config?.issuer ?? "",
    clientId: config?.client_id ?? "",
    clientSecret: "",
    scopes: (config?.scopes ?? ["openid", "profile", "email"]).join(" "),
    adminClaim: config?.admin_claim ?? "groups",
    adminValue: config?.admin_value ?? "",
    memberValue: config?.member_value ?? "",
    readonlyValue: config?.readonly_value ?? "",
    requiredValue: config?.required_value ?? "",
    postLoginUrl: config?.post_login_url ?? "",
  };
}

function OidcProviderForm({ config }: { config: OidcConfigView | null }) {
  const t = useT();
  const [f, setF] = useState<FieldState>(() => initialState(config));
  const [errors, setErrors] = useState<{ issuer?: string; clientId?: string; form?: string }>({});
  const [confirmingRemove, setConfirmingRemove] = useState(false);

  const put = usePutOidcConfig();
  const remove = useDeleteOidcConfig();

  const secretSet = config?.client_secret_set ?? false;
  // Editing the managed Keycloak realm's config would re-point this workspace's
  // sign-in; warn when the issuer looks like a provisioned realm.
  const managed = config != null && config.issuer.includes("/realms/");

  function set<K extends keyof FieldState>(key: K, value: string) {
    setF((prev) => ({ ...prev, [key]: value }));
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    const next: typeof errors = {};
    if (!f.issuer.trim()) next.issuer = t("ssoProvider.issuerRequired");
    if (!f.clientId.trim()) next.clientId = t("ssoProvider.clientIdRequired");
    if (Object.keys(next).length > 0) {
      setErrors(next);
      return;
    }
    setErrors({});
    try {
      await put.mutateAsync({
        issuer: f.issuer.trim(),
        client_id: f.clientId.trim(),
        client_secret: f.clientSecret, // empty keeps the stored secret (backend)
        scopes: f.scopes.split(/\s+/).filter(Boolean),
        admin_claim: f.adminClaim.trim(),
        admin_value: f.adminValue.trim(),
        member_value: f.memberValue.trim(),
        readonly_value: f.readonlyValue.trim(),
        required_value: f.requiredValue.trim() || null,
        post_login_url: f.postLoginUrl.trim() || null,
      });
      toast.success(t("ssoProvider.saveSuccess"));
      set("clientSecret", "");
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) {
        setErrors({ form: t("common.rateLimited") });
      } else {
        setErrors({ form: t("ssoProvider.saveError") });
      }
    }
  }

  async function handleConfirmRemove() {
    try {
      await remove.mutateAsync();
      toast.success(t("ssoProvider.removeSuccess"));
      setConfirmingRemove(false);
      setF(initialState(null));
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("ssoProvider.removeError"),
      );
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <Header />

      {managed && (
        <Card className="border-amber-500/40">
          <CardContent className="flex items-start gap-3 py-3 text-sm">
            <ShieldCheck className="mt-0.5 size-4 text-amber-500" aria-hidden="true" />
            <p className="text-muted-foreground">{t("ssoProvider.managedNote")}</p>
          </CardContent>
        </Card>
      )}

      {config == null && (
        <Card>
          <CardContent className="flex flex-col items-center gap-2 py-6 text-center">
            <p className="font-medium">{t("ssoProvider.empty")}</p>
            <p className="text-sm text-muted-foreground">{t("ssoProvider.emptyHint")}</p>
          </CardContent>
        </Card>
      )}

      <Card>
        <CardContent className="py-4">
          <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
            <Field label={t("ssoProvider.issuerLabel")} hint={t("ssoProvider.issuerHint")} error={errors.issuer} htmlFor="oidc-issuer">
              <Input id="oidc-issuer" value={f.issuer} onChange={(e) => set("issuer", e.target.value)} placeholder="https://acme.okta.com" aria-invalid={errors.issuer != null} />
            </Field>

            <Field label={t("ssoProvider.clientIdLabel")} error={errors.clientId} htmlFor="oidc-client-id">
              <Input id="oidc-client-id" value={f.clientId} onChange={(e) => set("clientId", e.target.value)} aria-invalid={errors.clientId != null} />
            </Field>

            <Field
              label={t("ssoProvider.clientSecretLabel")}
              hint={secretSet ? t("ssoProvider.clientSecretSetHint") : t("ssoProvider.clientSecretNewHint")}
              htmlFor="oidc-client-secret"
            >
              <Input
                id="oidc-client-secret"
                type="password"
                value={f.clientSecret}
                onChange={(e) => set("clientSecret", e.target.value)}
                placeholder={secretSet ? "••••••••" : ""}
                autoComplete="new-password"
              />
            </Field>

            <Field label={t("ssoProvider.scopesLabel")} hint={t("ssoProvider.scopesHint")} htmlFor="oidc-scopes">
              <Input id="oidc-scopes" value={f.scopes} onChange={(e) => set("scopes", e.target.value)} />
            </Field>

            <Field label={t("ssoProvider.adminClaimLabel")} hint={t("ssoProvider.adminClaimHint")} htmlFor="oidc-admin-claim">
              <Input id="oidc-admin-claim" value={f.adminClaim} onChange={(e) => set("adminClaim", e.target.value)} />
            </Field>

            <div className="grid gap-4 sm:grid-cols-3">
              <Field label={t("ssoProvider.adminValueLabel")} htmlFor="oidc-admin-value">
                <Input id="oidc-admin-value" value={f.adminValue} onChange={(e) => set("adminValue", e.target.value)} />
              </Field>
              <Field label={t("ssoProvider.memberValueLabel")} htmlFor="oidc-member-value">
                <Input id="oidc-member-value" value={f.memberValue} onChange={(e) => set("memberValue", e.target.value)} />
              </Field>
              <Field label={t("ssoProvider.readonlyValueLabel")} htmlFor="oidc-readonly-value">
                <Input id="oidc-readonly-value" value={f.readonlyValue} onChange={(e) => set("readonlyValue", e.target.value)} />
              </Field>
            </div>

            <Field label={t("ssoProvider.requiredValueLabel")} hint={t("ssoProvider.requiredValueHint")} htmlFor="oidc-required-value">
              <Input id="oidc-required-value" value={f.requiredValue} onChange={(e) => set("requiredValue", e.target.value)} />
            </Field>

            <Field label={t("ssoProvider.postLoginUrlLabel")} htmlFor="oidc-post-login">
              <Input id="oidc-post-login" value={f.postLoginUrl} onChange={(e) => set("postLoginUrl", e.target.value)} placeholder="https://app.example.com" />
            </Field>

            {errors.form && (
              <p className="text-sm text-destructive" role="alert">
                {errors.form}
              </p>
            )}

            <div className="flex flex-wrap items-center justify-between gap-2">
              <Button type="submit" disabled={put.isPending}>
                {put.isPending ? t("ssoProvider.saving") : t("ssoProvider.save")}
              </Button>
              {config != null && (
                <Button type="button" variant="outline" onClick={() => setConfirmingRemove(true)}>
                  {t("ssoProvider.remove")}
                </Button>
              )}
            </div>
          </form>
        </CardContent>
      </Card>

      <AlertDialog open={confirmingRemove} onOpenChange={(open) => !open && setConfirmingRemove(false)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("ssoProvider.removeTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("ssoProvider.removeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={remove.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={remove.isPending} onClick={handleConfirmRemove}>
              {remove.isPending ? t("ssoProvider.removing") : t("ssoProvider.remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

function Field({
  label,
  hint,
  error,
  htmlFor,
  children,
}: {
  label: string;
  hint?: string;
  error?: string;
  htmlFor: string;
  children: ReactNode;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={htmlFor}>{label}</Label>
      {children}
      {hint && !error && <p className="text-xs text-muted-foreground">{hint}</p>}
      {error && (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      )}
    </div>
  );
}
