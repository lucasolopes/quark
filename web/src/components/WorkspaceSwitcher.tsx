import { Check, ChevronsUpDown, Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { CreateWorkspaceForm } from "@/components/CreateWorkspaceForm";
import { useT } from "@/i18n";
import { useMe, useSwitchWorkspace } from "@/lib/queries";

/**
 * Header control (cloud only) to switch between the user's workspaces and to
 * create a new one via a dialog. Returns null in OSS (`me.memberships`
 * undefined) or before a workspace is selected.
 */
export function WorkspaceSwitcher() {
  const t = useT();
  const me = useMe();
  const switchWs = useSwitchWorkspace();
  const [createOpen, setCreateOpen] = useState(false);

  const memberships = me.data?.memberships;
  const current = me.data?.current_tenant;
  if (!memberships || current == null) return null;
  const currentName = memberships.find((m) => m.tenant_id === current)?.name ?? "";

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            <Button variant="outline" size="sm" className="max-w-[12rem] justify-between gap-2">
              <span className="truncate">{currentName}</span>
              <ChevronsUpDown className="size-3.5 shrink-0 opacity-60" aria-hidden="true" />
            </Button>
          }
        />
        <DropdownMenuContent align="end" className="w-56">
          <DropdownMenuGroup>
            <DropdownMenuLabel>{t("shell.workspaceLabel")}</DropdownMenuLabel>
            {memberships.map((m) => (
              <DropdownMenuItem
                key={m.tenant_id}
                disabled={switchWs.isPending || m.tenant_id === current}
                onClick={() => { if (m.tenant_id !== current) switchWs.mutate(m.tenant_id); }}
              >
                <Check className={m.tenant_id === current ? "size-4" : "size-4 opacity-0"} aria-hidden="true" />
                <span className="truncate">{m.name}</span>
              </DropdownMenuItem>
            ))}
          </DropdownMenuGroup>
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setCreateOpen(true)}>
            <Plus className="size-4" aria-hidden="true" />
            {t("shell.createWorkspace")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <Dialog open={createOpen} onOpenChange={setCreateOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("onboarding.title")}</DialogTitle>
            <DialogDescription>{t("onboarding.description")}</DialogDescription>
          </DialogHeader>
          <CreateWorkspaceForm onCreated={() => setCreateOpen(false)} />
        </DialogContent>
      </Dialog>
    </>
  );
}
