import { Loader2, Lock } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useMutation } from "@tanstack/react-query";
import { toast } from "sonner";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError, api, oidcLoginUrl } from "@/lib/api";
import { clearToken, setToken } from "@/lib/auth";

export function Login() {
  const t = useT();
  const [value, setValue] = useState("");
  const [oidcEnabled, setOidcEnabled] = useState(false);
  // Optional operator-configured label for the shared OIDC button
  // (`QUARK_OIDC_BUTTON_LABEL`); null means fall back to the i18n label.
  const [oidcButtonLabel, setOidcButtonLabel] = useState<string | null>(null);
  const [multiTenant, setMultiTenant] = useState(false);
  // Whether the server has a break-glass admin token configured. Defaults to
  // true so the field shows until `/admin/me` says otherwise (and stays visible
  // against older servers that omit the flag).
  const [adminLoginEnabled, setAdminLoginEnabled] = useState(true);
  const [email, setEmail] = useState("");
  // Set once email discovery comes back with no org: reveals the shared
  // provider button instead of blocking the user behind the email step.
  const [sharedLoginRevealed, setSharedLoginRevealed] = useState(false);
  const inputRef = useRef<HTMLInputElement>(null);
  const navigate = useNavigate();
  const [params] = useSearchParams();
  // Untrusted UX hint from the URL: only ever displayed and forwarded to
  // `oidcLoginUrl`, never validated here — the server decides what it means.
  const org = params.get("org")?.trim() || "";
  // `?org=` always wins (LUC-53): it already picked the tenant, so the
  // email-first discovery step would be redundant. The step is cloud-only:
  // home-realm discovery is meaningless in single-tenant OSS, so it never
  // shows there even when a shared OIDC provider is configured.
  const showEmailStage = oidcEnabled && multiTenant && !org && !sharedLoginRevealed;

  useEffect(() => {
    inputRef.current?.focus();
    // Detect an existing session (e.g. right after the OIDC callback redirect)
    // and whether the provider button should be shown.
    let alive = true;
    api
      .me()
      .then((me) => {
        if (!alive) return;
        if (me.authenticated) navigate("/links", { replace: true });
        else {
          setOidcEnabled(me.oidc_enabled);
          setOidcButtonLabel(me.oidc_button_label ?? null);
          setMultiTenant(me.multi_tenant ?? false);
          setAdminLoginEnabled(me.admin_login_enabled ?? true);
        }
      })
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [navigate]);

  const mutation = useMutation({
    mutationFn: async (token: string) => {
      setToken(token);
      await api.listLinks({ limit: 1 });
    },
    onSuccess: () => {
      toast.success(t("login.sessionStarted"));
      navigate("/links", { replace: true });
    },
    onError: () => {
      clearToken();
    },
  });

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const token = value.trim();
    if (!token || mutation.isPending) return;
    mutation.mutate(token);
  }

  const discoverMutation = useMutation({
    mutationFn: (candidate: string) => api.discoverSso(candidate),
    onSuccess: (result) => {
      if (result.org) {
        window.location.href = oidcLoginUrl(result.org, email);
      } else {
        // No SSO org for this domain: stop blocking on the email step and
        // let the visitor reach the shared login options.
        setSharedLoginRevealed(true);
      }
    },
    onError: () => {
      // Discovery is a convenience, not a gate: if it fails, fall back to
      // the shared login options instead of stranding the visitor.
      setSharedLoginRevealed(true);
    },
  });

  function handleEmailSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    const candidate = email.trim();
    if (!candidate || discoverMutation.isPending) return;
    discoverMutation.mutate(candidate);
  }

  return (
    <div className="relative min-h-svh overflow-hidden bg-background">
      {/* Backdrop do DS pra páginas fora do Shell (login, onboarding, convite):
          glow lime + dot-grid mascarado (utilities .bg-hero-glow/.bg-dot-grid
          do index.css). z-index negativo: ficam atrás do conteúdo em fluxo
          normal sem depender da ordem no DOM em relação ao LanguageSwitcher. */}
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-hero-glow" />
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-dot-grid" />
      <div className="absolute right-4 top-4">
        <LanguageSwitcher />
      </div>
      <div className="flex min-h-svh items-center justify-center p-4">
        <div className="w-full max-w-[400px] animate-rise">
          <div className="mb-[30px] flex flex-col items-center text-center">
            <QuarkMark className="mb-[18px] size-[42px] text-primary drop-shadow-[0_0_14px_rgba(198,249,78,0.4)]" />
            <h1 className="font-heading text-[26px] font-bold tracking-display text-strong">{t("login.title")}</h1>
            <p className="mt-2 text-[14.5px] text-muted-foreground">
              {adminLoginEnabled ? t("login.description") : t("login.descriptionSso")}
            </p>
          </div>
          <div className="w-full rounded-2xl border border-input bg-card p-6 shadow-modal">
            {oidcEnabled && (
              <>
                {org ? (
                  <>
                    <p className="mb-2 text-center text-sm text-muted-foreground">
                      {t("login.orgHeader", { org })}
                    </p>
                    <Button
                      type="button"
                      className="w-full"
                      onClick={() => {
                        window.location.href = oidcLoginUrl(org, email);
                      }}
                    >
                      {t("login.orgButton", { org })}
                    </Button>
                  </>
                ) : showEmailStage ? (
                  <form onSubmit={handleEmailSubmit} className="flex flex-col gap-3" noValidate>
                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="sso-email" className="text-sm font-medium">
                        {t("login.emailLabel")}
                      </label>
                      <Input
                        id="sso-email"
                        type="email"
                        autoComplete="email"
                        placeholder="jane@acme.com"
                        value={email}
                        onChange={(e) => setEmail(e.target.value)}
                      />
                      <p className="text-xs text-muted-foreground">{t("login.emailHint")}</p>
                    </div>
                    <Button type="submit" variant="outline" disabled={!email.trim() || discoverMutation.isPending}>
                      {discoverMutation.isPending && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
                      {t("login.continue")}
                    </Button>
                  </form>
                ) : (
                  <Button
                    type="button"
                    className="w-full"
                    onClick={() => {
                      window.location.href = oidcLoginUrl(undefined, email);
                    }}
                  >
                    {oidcButtonLabel ?? t("login.oidcButton")}
                  </Button>
                )}
              </>
            )}
            {oidcEnabled && adminLoginEnabled && (
              <div className="my-4 flex items-center gap-3 text-xs text-muted-foreground">
                <span className="h-px flex-1 bg-border" />
                {t("login.or")}
                <span className="h-px flex-1 bg-border" />
              </div>
            )}
            {adminLoginEnabled && (
              <form
                onSubmit={handleSubmit}
                noValidate
                className="flex flex-col gap-3"
              >
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="admin-token" className="text-sm font-medium">
                    {t("login.tokenLabel")}
                  </label>
                  <Input
                    id="admin-token"
                    ref={inputRef}
                    type="password"
                    autoComplete="off"
                    spellCheck={false}
                    placeholder="••••••••"
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                    aria-invalid={mutation.isError}
                    aria-describedby={mutation.isError ? "admin-token-error" : undefined}
                    className="font-mono text-[13px]"
                  />
                  {mutation.isError && (
                    <p id="admin-token-error" role="alert" className="text-sm text-destructive">
                      {mutation.error instanceof ApiError && mutation.error.status === 401
                        ? t("login.invalidToken")
                        : t("login.connectionError")}
                    </p>
                  )}
                </div>
                <Button
                  type="submit"
                  variant="outline"
                  disabled={!value.trim() || mutation.isPending}
                  className="mt-1 font-mono text-[13px]"
                >
                  {mutation.isPending ? (
                    <Loader2 className="size-4 animate-spin" aria-hidden="true" />
                  ) : (
                    <Lock className="size-4" aria-hidden="true" />
                  )}
                  {t("login.submit")}
                </Button>
              </form>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
