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
import { PageHeader } from "@/components/PageHeader";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { formatDateTime } from "@/lib/format";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useCreateInvite, useInvites, useMe, useRevokeInvite } from "@/lib/queries";
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

/**
 * Avatar hues for member rows, drawn from the DS chart tokens (`--chart-2`, `--chart-3`).
 * `--chart-1` is skipped on purpose: it doubles as `--primary`, the brand action color, which
 * `tag-color.ts` also keeps scarce rather than spending it on a round-robin of decorative swatches.
 * `--chart-4` is skipped: it renders as the danger/destructive red, so an avatar in that hue reads
 * as an error or "at risk" state rather than a neutral identity color.
 * `--chart-5` is skipped: it lacks sufficient contrast (3.3:1) in light theme vs. dark text (#0a0b0f), failing WCAG AA (4.5:1).
 */
const AVATAR_HUES = ["var(--chart-2)", "var(--chart-3)"] as const;

/**
 * Small stable string hash (no crypto needed): the same email always lands on the same hue, with
 * no randomness, so a pending invite's avatar color never changes between renders or reloads.
 */
function hashSeed(seed: string): number {
  let h = 0;
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) >>> 0;
  return h;
}

/** Deterministic avatar background for a member row, keyed off the invite's email. */
function avatarHue(email: string): string {
  return AVATAR_HUES[hashSeed(email.toLowerCase()) % AVATAR_HUES.length];
}

/**
 * Avatar initials for a member row. An invite carries no display name (it is only ever a pending
 * placeholder until accepted), so this takes the email's first character — the same one-token
 * shape `Shell`'s `initialsFrom` derives for an email `display` elsewhere in the panel.
 */
function emailInitials(email: string): string {
  return email.trim().slice(0, 1).toUpperCase();
}

const EMAIL_RE = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;

export function Members() {
  const t = useT();
  const [createOpen, setCreateOpen] = useState(false);
  const [revokingInvite, setRevokingInvite] = useState<InviteView | null>(null);
  const [createdInvite, setCreatedInvite] = useState<{ token: string; email: string } | null>(null);
  const [justCopiedLink, setJustCopiedLink] = useState(false);

  const query = useInvites();
  const revokeInvite = useRevokeInvite();
  const me = useMe();
  // With an external IdP (Keycloak) the invited user is onboarded by an emailed
  // set-password link; the `/invite/<token>` link never onboards a new user in
  // that mode, so we confirm "email sent" instead of offering the dead link.
  const ssoProvisioning = me.data?.sso_provisioning ?? false;
  const createdLink = createdInvite ? `${window.location.origin}/invite/${createdInvite.token}` : null;

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
    <div className="flex flex-col gap-4 animate-rise max-w-[860px]">
      <PageHeader
        title={t("invites.title")}
        subtitle={t("invites.subtitle")}
        actions={
          <Button onClick={() => setCreateOpen(true)}>
            <Plus className="size-4" />
            {t("invites.inviteButton")}
          </Button>
        }
      />

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
            <div className="flex size-10 items-center justify-center rounded-[9px] bg-secondary">
              <Users className="size-[18px] text-muted-foreground" aria-hidden="true" />
            </div>
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
        <ul className="overflow-hidden rounded-lg border border-border bg-card shadow-card" aria-label={t("invites.title")}>
          {invites.map((invite) => (
            <li
              key={invite.id}
              data-testid="member-row"
              className="flex flex-wrap items-center gap-3.5 border-b border-border px-4 py-4 last:border-b-0"
            >
              <div
                aria-hidden="true"
                className="flex size-9 shrink-0 items-center justify-center rounded-full font-heading text-[13px] font-bold text-primary-foreground"
                style={{ backgroundColor: avatarHue(invite.email) }}
              >
                {emailInitials(invite.email)}
              </div>
              <div className="min-w-0 flex-1">
                <div className="truncate text-[14.5px] font-semibold text-strong">{invite.email}</div>
                <div className="truncate text-[12.5px] text-muted-foreground">
                  {t("invites.pending")} · {t("invites.expires", { date: formatDateTime(invite.expires) })}
                </div>
              </div>
              <span className="shrink-0 rounded-full border border-border px-3 py-1 text-[12.5px] text-muted-foreground">
                {roleLabel(t, invite.role)}
              </span>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label={t("invites.revoke") + " " + invite.email}
                onClick={() => setRevokingInvite(invite)}
              >
                <Trash2 className="size-3.5" />
              </Button>
            </li>
          ))}
        </ul>
      )}

      <CreateInviteDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={(token, email) => setCreatedInvite({ token, email })}
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
        open={createdInvite != null}
        onOpenChange={(open) => {
          if (!open) setCreatedInvite(null);
        }}
      >
        <DialogContent>
          {ssoProvisioning ? (
            <>
              <DialogHeader>
                <DialogTitle>{t("invites.emailSentTitle")}</DialogTitle>
                <DialogDescription>
                  {t("invites.emailSentBody", { email: createdInvite?.email ?? "" })}
                </DialogDescription>
              </DialogHeader>
              <DialogFooter>
                <Button type="button" onClick={() => setCreatedInvite(null)}>
                  {t("common.cancel")}
                </Button>
              </DialogFooter>
            </>
          ) : (
            <>
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
                <Button type="button" onClick={() => setCreatedInvite(null)}>
                  {t("common.cancel")}
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
    </div>
  );
}

function MembersSkeleton() {
  return (
    <div className="overflow-hidden rounded-lg border border-border" aria-hidden="true">
      {Array.from({ length: 4 }).map((_, i) => (
        <div key={i} className="flex items-center gap-3.5 border-b border-border px-4 py-4 last:border-b-0">
          <Skeleton className="size-9 shrink-0 rounded-full" />
          <div className="flex-1">
            <Skeleton className="h-3.5 w-40" />
            <Skeleton className="mt-2 h-3 w-56" />
          </div>
          <Skeleton className="h-6 w-16 shrink-0 rounded-full" />
        </div>
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
  /** Called with the raw token and the invited email right after a successful creation, before the dialog closes. */
  onCreated: (token: string, email: string) => void;
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
      const invitedEmail = email.trim();
      const result = await createInvite.mutateAsync({ email: invitedEmail, role });
      toast.success(t("invites.createdSuccess"));
      reset();
      onOpenChange(false);
      onCreated(result.token, invitedEmail);
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
