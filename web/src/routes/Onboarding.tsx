import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { CreateWorkspaceForm } from "@/components/CreateWorkspaceForm";
import { OutOfShellFrame } from "@/components/OutOfShellFrame";
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
 * Out-of-Shell page: shares `OutOfShellFrame` (glow + dot-grid, centered
 * glyph/title header, hairline card) with `Login`/`AcceptInvite`, rather than
 * the in-Shell `PageHeader` chrome, since this screen renders before a
 * workspace — and therefore the Shell itself — exists.
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
    <OutOfShellFrame
      title={hasExisting ? t("onboarding.chooseTitle") : t("onboarding.title")}
      subtitle={t("onboarding.description")}
      topRight={<LanguageSwitcher />}
    >
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
    </OutOfShellFrame>
  );
}
