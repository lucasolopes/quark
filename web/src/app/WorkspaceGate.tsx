import { Loader2 } from "lucide-react";
import { useEffect } from "react";
import { Onboarding } from "@/routes/Onboarding";
import { useSwitchWorkspace } from "@/lib/queries";
import type { MeResponse } from "@/lib/types";

/**
 * Rendered by `RequireAuth` for a cloud user with no current workspace. With
 * exactly one membership it auto-switches into it (a returning single-workspace
 * user should not have to click); with zero or several it shows `Onboarding`.
 * If the auto-switch fails (e.g. rate-limited or a backend blip) it falls
 * through to `Onboarding`, where the workspace is a clickable retry, rather
 * than stranding the user on the spinner.
 */
export function WorkspaceGate({ me }: { me: MeResponse }) {
  const memberships = me.memberships ?? [];
  const only = memberships.length === 1 ? memberships[0].tenant_id : null;
  const switchWs = useSwitchWorkspace();

  useEffect(() => {
    if (only != null && switchWs.isIdle) switchWs.mutate(only);
    // Fire once for the single-membership case; the mutation's own state guards re-entry.
  }, [only, switchWs]);

  if (only != null && !switchWs.isError) {
    return (
      <div className="flex min-h-svh items-center justify-center bg-background">
        <Loader2 className="size-6 animate-spin text-muted-foreground" aria-label="Loading" />
      </div>
    );
  }
  return <Onboarding memberships={memberships} />;
}
