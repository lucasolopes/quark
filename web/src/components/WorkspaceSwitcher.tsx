import { Check, ChevronsUpDown, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "@/components/ui/dialog";
import {
  DropdownMenu, DropdownMenuContent, DropdownMenuGroup, DropdownMenuItem, DropdownMenuLabel,
  DropdownMenuSeparator, DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { CreateWorkspaceForm } from "@/components/CreateWorkspaceForm";
import { DeleteWorkspaceDialog } from "@/components/DeleteWorkspaceDialog";
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
  const [deleteOpen, setDeleteOpen] = useState(false);

  const memberships = me.data?.memberships;
  const current = me.data?.current_tenant;
  if (!memberships || current == null) return null;
  const currentMembership = memberships.find((m) => m.tenant_id === current);
  const currentName = currentMembership?.name ?? "";
  // Roles arrive from `/admin/me` as serde snake_case ("owner", "admin", …);
  // lowercased here so a serialization change in casing does not silently
  // hide the item. Only the Owner may delete, and the server enforces it too.
  const isOwner = currentMembership?.role.toLowerCase() === "owner";

  return (
    <>
      <DropdownMenu>
        <DropdownMenuTrigger
          render={
            // Sits in the sidebar's card slot (Shell v2) — full width, taller
            // padding and a plain `border-border` to read as a card rather
            // than a compact toolbar button. Only the shape changed here;
            // selection/creation logic is untouched.
            <Button
              variant="outline"
              size="sm"
              className="h-auto w-full justify-between gap-2 rounded-[10px] border-border p-2.5 font-semibold"
            >
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
          {isOwner && (
            <DropdownMenuItem variant="destructive" onClick={() => setDeleteOpen(true)}>
              <Trash2 className="size-4" aria-hidden="true" />
              {t("workspaceDelete.menuItem")}
            </DropdownMenuItem>
          )}
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
      {currentMembership && isOwner && (
        <DeleteWorkspaceDialog
          open={deleteOpen}
          onOpenChange={setDeleteOpen}
          tenantId={currentMembership.tenant_id}
          name={currentMembership.name}
          slug={currentMembership.slug}
        />
      )}
    </>
  );
}
