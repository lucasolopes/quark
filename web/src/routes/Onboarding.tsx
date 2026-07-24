import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { CreateWorkspaceForm } from "@/components/CreateWorkspaceForm";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { Button } from "@/components/ui/button";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useSwitchWorkspace } from "@/lib/queries";
import type { Membership } from "@/lib/types";

/**
 * Full-screen gate shown to a cloud user with no current workspace. With
 * existing memberships it lists them (pick one to switch); it always offers the
 * create-workspace form below. `RequireAuth` renders this; there is no route.
 *
 * Out-of-Shell page: shares the Login v2 backdrop (glow + dot-grid, centered
 * glyph/title header, hairline card) rather than the in-Shell `PageHeader`
 * chrome, since this screen renders before a workspace — and therefore the
 * Shell itself — exists.
 */
export function Onboarding({ memberships }: { memberships: Membership[] }) {
  const t = useT();
  const switchWs = useSwitchWorkspace();
  const hasExisting = memberships.length > 0;

  const switchErrorText =
    switchWs.error instanceof ApiError && switchWs.error.status === 429
      ? t("common.rateLimited")
      : switchWs.isError
        ? t("onboarding.switchError")
        : null;

  return (
    <div className="relative min-h-svh overflow-hidden bg-background">
      {/* Backdrop do DS pra páginas fora do Shell (login, onboarding, convite):
          mesmas utilities da Task 9 (glow lime + dot-grid), pra manter as três
          telas fora do Shell com a mesma moldura visual. */}
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-hero-glow" />
      <div aria-hidden="true" className="pointer-events-none absolute inset-0 -z-10 bg-dot-grid" />
      <div className="absolute right-4 top-4">
        <LanguageSwitcher />
      </div>
      <div className="flex min-h-svh items-center justify-center p-4">
        <div className="w-full max-w-[400px] animate-rise">
          <div className="mb-[30px] flex flex-col items-center text-center">
            <QuarkMark className="mb-[18px] size-[42px] text-primary drop-shadow-[0_0_14px_rgba(198,249,78,0.4)]" />
            <h1 className="font-heading text-[26px] font-bold tracking-display text-strong">
              {hasExisting ? t("onboarding.chooseTitle") : t("onboarding.title")}
            </h1>
            <p className="mt-2 text-[14.5px] text-muted-foreground">{t("onboarding.description")}</p>
          </div>
          <div className="w-full rounded-2xl border border-input bg-card p-6 shadow-modal">
            {hasExisting && (
              <div className="mb-4 flex flex-col gap-2">
                {memberships.map((m) => (
                  <Button
                    key={m.tenant_id}
                    variant="outline"
                    className="justify-between"
                    disabled={switchWs.isPending}
                    onClick={() => switchWs.mutate(m.tenant_id)}
                  >
                    <span className="truncate">{m.name}</span>
                    <span className="font-mono text-xs text-muted-foreground">{m.role}</span>
                  </Button>
                ))}
                {switchErrorText && (
                  <p role="alert" className="text-sm text-destructive">{switchErrorText}</p>
                )}
                <div className="my-1 flex items-center gap-3 text-xs text-muted-foreground">
                  <span className="h-px flex-1 bg-border" />
                  {t("onboarding.orCreate")}
                  <span className="h-px flex-1 bg-border" />
                </div>
              </div>
            )}
            <CreateWorkspaceForm />
          </div>
        </div>
      </div>
    </div>
  );
}
