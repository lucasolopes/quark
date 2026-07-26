import { Loader2, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogMedia,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useDeleteWorkspace } from "@/lib/queries";
import { FIELD_LABEL_CLASS } from "@/lib/utils";

interface DeleteWorkspaceDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  tenantId: number;
  name: string;
  slug: string;
}

/**
 * Type-the-slug confirmation for deleting a workspace (cloud only). The
 * confirm button stays disabled until the typed text equals the slug exactly,
 * because the action is irreversible and takes every link, click and
 * integration of the workspace with it: a distracted click must not be enough.
 *
 * Rendered by `WorkspaceSwitcher` only when the current workspace's role is
 * owner. The server enforces the same rule (403 otherwise); the menu gate is
 * there so the option is not offered to someone who cannot use it.
 */
export function DeleteWorkspaceDialog({ open, onOpenChange, tenantId, name, slug }: DeleteWorkspaceDialogProps) {
  const t = useT();
  const [typed, setTyped] = useState("");
  const mutation = useDeleteWorkspace();
  const { reset } = mutation;

  // Reopening must not inherit the previous attempt's text or error, otherwise
  // a dialog could open with the confirm button already enabled.
  useEffect(() => {
    if (!open) { setTyped(""); reset(); }
  }, [open, reset]);

  const matches = typed === slug;

  const errorText =
    mutation.error instanceof ApiError && mutation.error.status === 409
      ? t("workspaceDelete.lastWorkspace")
      : mutation.error instanceof ApiError && mutation.error.status === 403
        ? t("workspaceDelete.notOwner")
        : mutation.isError
          ? t("workspaceDelete.error")
          : null;

  function handleConfirm() {
    if (!matches || mutation.isPending) return;
    mutation.mutate(tenantId, { onSuccess: () => onOpenChange(false) });
  }

  return (
    <AlertDialog open={open} onOpenChange={onOpenChange}>
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogMedia>
            <Trash2 className="text-destructive" aria-hidden="true" />
          </AlertDialogMedia>
          <AlertDialogTitle>{t("workspaceDelete.title", { name })}</AlertDialogTitle>
          <AlertDialogDescription>{t("workspaceDelete.description")}</AlertDialogDescription>
        </AlertDialogHeader>
        <div className="flex flex-col gap-1.5">
          <label htmlFor="ws-delete-confirm" className={FIELD_LABEL_CLASS}>
            {t("workspaceDelete.confirmLabel", { slug })}
          </label>
          <Input
            id="ws-delete-confirm"
            value={typed}
            onChange={(e) => setTyped(e.target.value)}
            className="font-mono"
            autoComplete="off"
            aria-invalid={mutation.isError}
            aria-describedby={mutation.isError ? "ws-delete-error" : undefined}
          />
          {errorText && (
            <p id="ws-delete-error" role="alert" className="text-sm text-destructive">{errorText}</p>
          )}
        </div>
        <AlertDialogFooter>
          <AlertDialogCancel disabled={mutation.isPending}>{t("common.cancel")}</AlertDialogCancel>
          <AlertDialogAction variant="destructive" disabled={!matches || mutation.isPending} onClick={handleConfirm}>
            {mutation.isPending && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
            {mutation.isPending ? t("workspaceDelete.deleting") : t("workspaceDelete.confirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
