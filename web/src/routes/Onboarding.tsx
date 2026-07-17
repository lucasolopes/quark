import { LanguageSwitcher } from "@/components/LanguageSwitcher";
import { CreateWorkspaceForm } from "@/components/CreateWorkspaceForm";
import { QuarkMark } from "@/components/brand/QuarkMark";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useSwitchWorkspace } from "@/lib/queries";
import type { Membership } from "@/lib/types";

/**
 * Full-screen gate shown to a cloud user with no current workspace. With
 * existing memberships it lists them (pick one to switch); it always offers the
 * create-workspace form below. `RequireAuth` renders this; there is no route.
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
    <div className="flex min-h-svh items-center justify-center bg-background p-4">
      <div className="absolute right-4 top-4"><LanguageSwitcher /></div>
      <Card className="w-full max-w-sm">
        <CardHeader>
          <div className="mb-1 flex items-center gap-3">
            <QuarkMark className="size-8 text-primary drop-shadow-[0_0_10px_rgba(198,249,78,0.55)]" />
            <CardTitle className="font-heading text-2xl font-bold tracking-tight">
              {hasExisting ? t("onboarding.chooseTitle") : t("onboarding.title")}
            </CardTitle>
          </div>
          <CardDescription>{t("onboarding.description")}</CardDescription>
        </CardHeader>
        <CardContent className="flex flex-col gap-4">
          {hasExisting && (
            <div className="flex flex-col gap-2">
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
        </CardContent>
      </Card>
    </div>
  );
}
