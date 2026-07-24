import { AlertTriangle, Globe, Plus, RotateCw, Trash2 } from "lucide-react";
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
import { Badge } from "@/components/ui/badge";
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
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { formatDateTime } from "@/lib/format";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useCreateDomain, useDeleteDomain, useDomains, useMe, useSetPrimaryDomain, useVerifyDomain } from "@/lib/queries";
import type { LinkDomainView } from "@/lib/types";

const HOST_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/i;

/**
 * Admin UI for custom link domains (LUC-82): a workspace serves its short
 * links on its own host (e.g. `go.acme.com`) after DNS verification. The
 * tenant's automatic `<slug>.<suffix>` subdomain also shows here, flagged
 * "Automatic" and unremovable (it is re-seeded on boot). Cloud-only and
 * Owner/Admin-only (the nav item is gated on `full`).
 */
export function Domains() {
  const me = useMe();
  const cloud = me.data?.memberships !== undefined;

  if (!cloud) return null;

  return <DomainsPanel />;
}

function DomainsPanel() {
  const t = useT();
  const [addOpen, setAddOpen] = useState(false);
  const [verifyingId, setVerifyingId] = useState<number | null>(null);
  const [removingDomain, setRemovingDomain] = useState<LinkDomainView | null>(null);

  const me = useMe();
  const query = useDomains();
  const verifyDomain = useVerifyDomain();
  const deleteDomain = useDeleteDomain();
  const setPrimary = useSetPrimaryDomain();
  const [settingPrimaryId, setSettingPrimaryId] = useState<number | null>(null);

  const domains = query.data ?? [];

  async function handleSetPrimary(domain: LinkDomainView) {
    setSettingPrimaryId(domain.id);
    try {
      await setPrimary.mutateAsync(domain.id);
      toast.success(t("domains.setPrimarySuccess"));
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(t("domains.setPrimaryError"));
    } finally {
      setSettingPrimaryId(null);
    }
  }

  // The automatic subdomain (`<slug>.<suffix>`) is managed by the server, so
  // it is shown read-only (no verify/remove) and labelled. Derived from `me`,
  // not a server flag, since a verified custom domain looks the same otherwise.
  const suffix = me.data?.tenant_domain_suffix ?? null;
  const slug = me.data?.memberships?.find((m) => m.tenant_id === me.data?.current_tenant)?.slug;
  const autoHost = suffix && slug ? `${slug}.${suffix}` : null;

  async function handleVerify(domain: LinkDomainView) {
    setVerifyingId(domain.id);
    try {
      const result = await verifyDomain.mutateAsync(domain.id);
      if (result.status === "verified") {
        toast.success(t("domains.verifySuccess"));
      } else {
        toast(t("domains.verifyStillPending"));
      }
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(t("domains.verifyGenericError"));
    } finally {
      setVerifyingId(null);
    }
  }

  async function handleConfirmRemove() {
    if (!removingDomain) return;
    try {
      await deleteDomain.mutateAsync(removingDomain.id);
      toast.success(t("domains.removeSuccess"));
      setRemovingDomain(null);
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("domains.removeGenericError"),
      );
    }
  }

  return (
    <div className="flex flex-col gap-4 animate-rise">
      <PageHeader
        title={t("domains.title")}
        subtitle={t("domains.subtitle")}
        actions={
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="size-4" />
            {t("domains.addButton")}
          </Button>
        }
      />

      {query.isPending && (
        <div className="flex flex-col gap-2" aria-hidden="true">
          {Array.from({ length: 3 }).map((_, i) => (
            <Skeleton key={i} className="h-16 w-full" />
          ))}
        </div>
      )}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("domains.loadError")}</p>
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

      {!query.isPending && !query.isError && domains.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <div className="flex size-10 items-center justify-center rounded-[9px] bg-secondary">
              <Globe className="size-[18px] text-muted-foreground" aria-hidden="true" />
            </div>
            <div>
              <p className="font-medium">{t("domains.empty")}</p>
            </div>
            <Button onClick={() => setAddOpen(true)}>
              <Plus className="size-4" />
              {t("domains.addButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && domains.length > 0 && (
        <ul className="flex flex-col gap-2.5">
          {domains.map((domain) => {
            const isAuto = autoHost != null && domain.host === autoHost;
            const verified = domain.status === "verified";
            // Set-primary is offered on any verified domain (including the auto
            // subdomain) that is not already primary; verify/remove stay
            // restricted to custom (non-auto) domains, matching the pre-v2 rules.
            const canSetPrimary = verified && !domain.primary;
            const canVerify = !isAuto && domain.status === "pending";
            const canRemove = !isAuto;
            const hasActions = canSetPrimary || canVerify || canRemove;
            return (
              <li
                key={domain.id}
                data-testid="domain-card"
                className="card-hover flex flex-col gap-3 rounded-lg border border-border bg-card p-4 shadow-card"
              >
                <div className="flex items-center gap-3.5">
                  <div className="flex size-10 shrink-0 items-center justify-center rounded-[9px] bg-secondary">
                    <Globe className="size-[18px] text-muted-foreground" aria-hidden="true" />
                  </div>

                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-mono text-[15px] font-medium text-strong">{domain.host}</span>
                      {isAuto && <Badge variant="secondary">{t("domains.autoBadge")}</Badge>}
                      {domain.primary && <Badge>{t("domains.primaryBadge")}</Badge>}
                    </div>
                    <div className="mt-1 text-[12.5px] text-muted-foreground">{formatDateTime(domain.created)}</div>
                  </div>

                  <div className="flex shrink-0 items-center gap-2">
                    <span
                      aria-hidden="true"
                      data-testid="domain-status-dot"
                      className={`size-2 rounded-full ${verified ? "bg-primary" : "bg-muted-foreground"}`}
                    />
                    <span className="text-[13px] text-foreground">
                      {verified ? t("domains.statusVerified") : t("domains.statusPending")}
                    </span>
                  </div>
                </div>

                {hasActions && (
                  <div className="flex flex-wrap items-center justify-end gap-1.5">
                    {canSetPrimary && (
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={t("domains.setPrimaryAria", { host: domain.host })}
                        disabled={settingPrimaryId === domain.id}
                        onClick={() => handleSetPrimary(domain)}
                      >
                        {t("domains.setPrimary")}
                      </Button>
                    )}
                    {canVerify && (
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={t("domains.verifyAria", { host: domain.host })}
                        disabled={verifyingId === domain.id}
                        onClick={() => handleVerify(domain)}
                      >
                        {verifyingId === domain.id ? t("domains.verifying") : t("domains.verify")}
                      </Button>
                    )}
                    {canRemove && (
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("domains.removeAria", { host: domain.host })}
                        onClick={() => setRemovingDomain(domain)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    )}
                  </div>
                )}

                {canVerify && (
                  <div className="flex flex-col gap-2 rounded-lg border border-border bg-secondary/60 p-3.5 text-sm">
                    <p className="text-muted-foreground">
                      {t("domains.verifyInstructions", { host: domain.host })}
                    </p>
                    <div className="grid gap-1 font-mono text-xs">
                      {domain.cname_target && (
                        <>
                          <div>
                            <span className="text-muted-foreground">{t("domains.cnameName")}: </span>
                            {domain.host}
                          </div>
                          <div className="break-all">
                            <span className="text-muted-foreground">{t("domains.cnameTarget")}: </span>
                            {domain.cname_target}
                          </div>
                        </>
                      )}
                      <div className="mt-1">
                        <span className="text-muted-foreground">{t("domains.txtName")}: </span>
                        {domain.txt_name}
                      </div>
                      <div className="break-all">
                        <span className="text-muted-foreground">{t("domains.txtValue")}: </span>
                        {domain.txt_value}
                      </div>
                    </div>
                    <p className="text-xs text-muted-foreground">{t("domains.tlsNote")}</p>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <AddDomainDialog open={addOpen} onOpenChange={setAddOpen} />

      <AlertDialog open={removingDomain != null} onOpenChange={(open) => !open && setRemovingDomain(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("domains.removeTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("domains.removeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteDomain.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={deleteDomain.isPending} onClick={handleConfirmRemove}>
              {deleteDomain.isPending ? t("domains.removing") : t("domains.remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

interface FormErrors {
  host?: string;
  form?: string;
}

function AddDomainDialog({ open, onOpenChange }: { open: boolean; onOpenChange: (open: boolean) => void }) {
  const t = useT();
  const [host, setHost] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const createDomain = useCreateDomain();

  function reset() {
    setHost("");
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    const trimmed = host.trim();
    if (!trimmed) {
      setErrors({ host: t("domains.hostRequired") });
      return;
    }
    if (!HOST_RE.test(trimmed)) {
      setErrors({ host: t("domains.createInvalidError") });
      return;
    }
    setErrors({});
    try {
      await createDomain.mutateAsync(trimmed);
      toast.success(t("domains.createdSuccess"));
      reset();
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 409) {
        setErrors({ form: t("domains.createConflictError") });
      } else if (err instanceof ApiError && err.status === 400) {
        setErrors({ form: t("domains.createInvalidError") });
      } else if (err instanceof ApiError && err.status === 429) {
        setErrors({ form: t("common.rateLimited") });
      } else {
        setErrors({ form: t("domains.createGenericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("domains.addButton")}</DialogTitle>
            <DialogDescription>{t("domains.subtitle")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="add-domain">{t("domains.hostLabel")}</Label>
              <Input
                id="add-domain"
                type="text"
                placeholder={t("domains.hostPlaceholder")}
                value={host}
                onChange={(e) => setHost(e.target.value)}
                aria-invalid={errors.host != null}
                autoFocus
              />
              {errors.host && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.host}
                </p>
              )}
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
            <Button type="submit" disabled={createDomain.isPending}>
              {createDomain.isPending ? t("domains.adding") : t("domains.add")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
