import { AlertTriangle, Check, Copy, Plus, RotateCw, Trash2, Users } from "lucide-react";
import { useState, type FormEvent } from "react";
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
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Skeleton } from "@/components/ui/skeleton";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { formatDateTime } from "@/lib/format";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useCreateInvite, useInvites, useRevokeInvite } from "@/lib/queries";
import type { InviteView } from "@/lib/types";

/** Roles invitable through this screen. Owner is never offered (there is exactly one path to it: transfer, out of scope here). */
const INVITE_ROLES = ["admin", "member", "viewer"] as const;
type InviteRole = (typeof INVITE_ROLES)[number];

const ROLE_LABEL_KEY: Record<string, MessageKey> = {
  admin: "invites.roleAdmin",
  member: "invites.roleMember",
  viewer: "invites.roleViewer",
};

/** Maps a role string from the API (lowercase: "owner"/"admin"/"member"/"viewer") to its i18n label. Unknown roles fall back to the raw string. */
function roleLabel(t: ReturnType<typeof useT>, role: string): string {
  const key = ROLE_LABEL_KEY[role];
  return key ? t(key) : role;
}

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function Members() {
  const t = useT();
  const [createOpen, setCreateOpen] = useState(false);
  const [revokingInvite, setRevokingInvite] = useState<InviteView | null>(null);
  const [createdLink, setCreatedLink] = useState<string | null>(null);
  const [justCopiedLink, setJustCopiedLink] = useState(false);

  const query = useInvites();
  const revokeInvite = useRevokeInvite();

  const invites = query.data ?? [];

  async function handleConfirmRevoke() {
    if (!revokingInvite) return;
    try {
      await revokeInvite.mutateAsync(revokingInvite.id);
      toast.success(t("invites.revokedSuccess"));
      setRevokingInvite(null);
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("invites.revokeGenericError"),
      );
    }
  }

  async function handleCopyLink() {
    if (!createdLink) return;
    try {
      await navigator.clipboard.writeText(createdLink);
      toast.success(t("invites.linkCopied"));
      setJustCopiedLink(true);
      setTimeout(() => setJustCopiedLink(false), 1500);
    } catch {
      toast.error(t("invites.copyFailed"));
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="font-heading text-2xl font-semibold">{t("invites.title")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("invites.subtitle")}</p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus className="size-4" />
          {t("invites.inviteButton")}
        </Button>
      </div>

      {query.isPending && <MembersSkeleton />}

      {query.isError && query.error instanceof ApiError && query.error.status === 403 && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <p className="font-medium">{t("invites.forbidden")}</p>
          </CardContent>
        </Card>
      )}

      {query.isError && !(query.error instanceof ApiError && query.error.status === 403) && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("invites.loadError")}</p>
              <p className="text-sm text-muted-foreground">
                {query.error instanceof Error ? query.error.message : t("common.retryHint")}
              </p>
            </div>
            <Button variant="outline" onClick={() => query.refetch()}>
              <RotateCw className="size-4" />
              {t("common.retry")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && invites.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <Users className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("invites.empty")}</p>
            </div>
            <Button onClick={() => setCreateOpen(true)}>
              <Plus className="size-4" />
              {t("invites.inviteButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && invites.length > 0 && (
        <Card className="py-0">
          <Table>
            <caption className="sr-only">{t("invites.title")}</caption>
            <TableHeader>
              <TableRow>
                <TableHead>{t("invites.columnEmail")}</TableHead>
                <TableHead>{t("invites.columnRole")}</TableHead>
                <TableHead>{t("invites.columnExpires")}</TableHead>
                <TableHead>{t("invites.columnCreated")}</TableHead>
                <TableHead>
                  <span className="sr-only">{t("linkTable.actionsSr")}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {invites.map((invite) => (
                <TableRow key={invite.id}>
                  <TableCell>{invite.email}</TableCell>
                  <TableCell>{roleLabel(t, invite.role)}</TableCell>
                  <TableCell className="text-muted-foreground">{formatDateTime(invite.expires)}</TableCell>
                  <TableCell className="text-muted-foreground">{formatDateTime(invite.created)}</TableCell>
                  <TableCell>
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("invites.revoke") + " " + invite.email}
                        onClick={() => setRevokingInvite(invite)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  </TableCell>
                </TableRow>
              ))}
            </TableBody>
          </Table>
        </Card>
      )}

      <CreateInviteDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={(token) => setCreatedLink(`${window.location.origin}/invite/${token}`)}
      />

      <AlertDialog open={revokingInvite != null} onOpenChange={(open) => !open && setRevokingInvite(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("invites.revokeTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("invites.revokeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={revokeInvite.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={revokeInvite.isPending}
              onClick={handleConfirmRevoke}
            >
              {revokeInvite.isPending ? t("invites.revoking") : t("invites.revoke")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog
        open={createdLink != null}
        onOpenChange={(open) => {
          if (!open) setCreatedLink(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("invites.createdSuccess")}</DialogTitle>
          </DialogHeader>
          <div className="flex flex-col gap-1.5 py-3">
            <Label htmlFor="invite-link">{t("invites.copyLink")}</Label>
            <div className="flex items-center gap-2">
              <Input id="invite-link" type="text" readOnly value={createdLink ?? ""} className="font-mono" />
              <Button
                type="button"
                variant="outline"
                size="icon"
                aria-label={t("invites.copyLink")}
                onClick={handleCopyLink}
              >
                {justCopiedLink ? <Check className="size-4 text-brand-ink" /> : <Copy className="size-4" />}
              </Button>
            </div>
          </div>
          <DialogFooter>
            <Button type="button" onClick={() => setCreatedLink(null)}>
              {t("common.cancel")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function MembersSkeleton() {
  return (
    <div className="flex flex-col gap-2" aria-hidden="true">
      {Array.from({ length: 4 }).map((_, i) => (
        <Skeleton key={i} className="h-10 w-full" />
      ))}
    </div>
  );
}

interface FormErrors {
  email?: string;
  form?: string;
}

interface CreateInviteDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called with the raw token right after a successful creation, before the dialog closes. */
  onCreated: (token: string) => void;
}

function CreateInviteDialog({ open, onOpenChange, onCreated }: CreateInviteDialogProps) {
  const t = useT();
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<InviteRole>("member");
  const [errors, setErrors] = useState<FormErrors>({});
  const createInvite = useCreateInvite();

  function reset() {
    setEmail("");
    setRole("member");
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!email.trim()) {
      next.email = t("invites.emailRequired");
    } else if (!EMAIL_RE.test(email.trim())) {
      next.email = t("invites.emailInvalid");
    }
    return next;
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    const nextErrors = validate();
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }
    setErrors({});
    try {
      const result = await createInvite.mutateAsync({ email: email.trim(), role });
      toast.success(t("invites.createdSuccess"));
      reset();
      onOpenChange(false);
      onCreated(result.token);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("invites.createGenericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("invites.inviteButton")}</DialogTitle>
            <DialogDescription>{t("invites.subtitle")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="create-invite-email">{t("invites.emailLabel")}</Label>
              <Input
                id="create-invite-email"
                type="email"
                value={email}
                onChange={(e) => setEmail(e.target.value)}
                aria-invalid={errors.email != null}
                autoFocus
              />
              {errors.email && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.email}
                </p>
              )}
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="create-invite-role">{t("invites.roleLabel")}</Label>
              <select
                id="create-invite-role"
                className="border-input bg-transparent flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-xs outline-none"
                value={role}
                onChange={(e) => setRole(e.target.value as InviteRole)}
              >
                {INVITE_ROLES.map((r) => (
                  <option key={r} value={r}>
                    {t(ROLE_LABEL_KEY[r])}
                  </option>
                ))}
              </select>
            </div>

            {errors.form && (
              <p className="text-sm text-destructive" role="alert">
                {errors.form}
              </p>
            )}
          </div>

          <DialogFooter>
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={createInvite.isPending}>
              {createInvite.isPending ? t("invites.creating") : t("invites.create")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
