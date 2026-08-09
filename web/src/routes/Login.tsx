import { Loader2, Lock } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useNavigate, useSearchParams } from "react-router-dom";
import { useMutation } from "@tanstack/react-query";
import { toast } from "sonner";
import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { OutOfShellFrame } from "@/components/OutOfShellFrame";
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
  const [params, setSearchParams] = useSearchParams();
  // Untrusted UX hint from the URL: only ever displayed and forwarded to
  // `oidcLoginUrl`, never validated here — the server decides what it means.
  const org = params.get("org")?.trim() || "";
  // Set by the operator's login redirect (LUC-41 phase 1) when a brand new
  // member tries to join a workspace already at its plan's member ceiling.
  // Read once into state (not derived from `params` on every render) so it
  // follows the same lifecycle as `mutation.isError` below: it shows until
  // the visitor tries again, then clears, instead of reappearing on every
  // back-navigation or refresh that restores the query string.
  const [memberLimitError, setMemberLimitError] = useState(
    () => params.get("error") === "member_limit_reached",
  );

  useEffect(() => {
    // Strip `?error=` from the URL once its value has been captured into
    // state above, so a refresh or a later back-navigation to this URL
    // doesn't re-trigger the alert.
    if (params.get("error") !== "member_limit_reached") return;
    const next = new URLSearchParams(params);
    next.delete("error");
    setSearchParams(next, { replace: true });
    // Runs once on mount: `params`/`setSearchParams` intentionally excluded
    // so this doesn't re-fire on every search-param change.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
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
    setMemberLimitError(false);
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
    setMemberLimitError(false);
    discoverMutation.mutate(candidate);
  }

  return (
    <OutOfShellFrame
      title={t("login.title")}
      subtitle={adminLoginEnabled ? t("login.description") : t("login.descriptionSso")}
      topRight={<LanguageSwitcher />}
    >
      {memberLimitError && (
        <p role="alert" className="mb-4 text-center text-sm text-destructive">
          {t("login.memberLimit")}
        </p>
      )}
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
        <form onSubmit={handleSubmit} noValidate className="flex flex-col gap-3">
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
    </OutOfShellFrame>
  );
}
