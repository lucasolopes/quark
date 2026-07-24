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
import { useCreateSsoDomain, useDeleteSsoDomain, useMe, useOidcConfigured, useSsoDomains, useVerifySsoDomain } from "@/lib/queries";
import type { SsoDomainView } from "@/lib/types";
import { FIELD_LABEL_CLASS } from "@/lib/utils";

const DOMAIN_RE = /^[a-z0-9]([a-z0-9-]*[a-z0-9])?(\.[a-z0-9]([a-z0-9-]*[a-z0-9])?)+$/i;

/**
 * Admin UI for the SSO email-domain discovery feature (LUC-57 Task 5):
 * verify the email domains that route a workspace's team straight into its
 * own SSO provider, skipping the shared login. Cloud-only (OSS never has
 * `memberships`) and only meaningful once the workspace has an OIDC
 * provider configured — with neither, this renders nothing rather than a
 * confusing dead end.
 */
export function SsoDomains() {
  const t = useT();
  const me = useMe();
  const cloud = me.data?.memberships !== undefined;
  const oidcConfigured = useOidcConfigured(cloud);

  if (!cloud) return null;

  if (oidcConfigured.isPending) {
    return (
      <div className="flex flex-col gap-2 max-w-[860px]" aria-hidden="true">
        <Skeleton className="h-10 w-full" />
      </div>
    );
  }

  if (!oidcConfigured.data) {
    return (
      <div className="flex flex-col gap-4 animate-rise max-w-[860px]">
        <PageHeader title={t("ssoDomains.title")} subtitle={t("ssoDomains.notConfigured")} />
      </div>
    );
  }

  return <SsoDomainsPanel />;
}

function SsoDomainsPanel() {
  const t = useT();
  const [addOpen, setAddOpen] = useState(false);
  const [verifyingId, setVerifyingId] = useState<number | null>(null);
  const [removingDomain, setRemovingDomain] = useState<SsoDomainView | null>(null);

  const query = useSsoDomains();
  const verifyDomain = useVerifySsoDomain();
  const deleteDomain = useDeleteSsoDomain();

  const domains = query.data ?? [];

  async function handleVerify(domain: SsoDomainView) {
    setVerifyingId(domain.id);
    try {
      const result = await verifyDomain.mutateAsync(domain.id);
      if (result.status === "verified") {
        toast.success(t("ssoDomains.verifySuccess"));
      } else {
        toast(t("ssoDomains.verifyStillPending"));
      }
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(t("ssoDomains.verifyGenericError"));
    } finally {
      setVerifyingId(null);
    }
  }

  async function handleConfirmRemove() {
    if (!removingDomain) return;
    try {
      await deleteDomain.mutateAsync(removingDomain.id);
      toast.success(t("ssoDomains.removeSuccess"));
      setRemovingDomain(null);
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("ssoDomains.removeGenericError"),
      );
    }
  }

  return (
    <div className="flex flex-col gap-4 animate-rise max-w-[860px]">
      <PageHeader
        title={t("ssoDomains.title")}
        subtitle={t("ssoDomains.subtitle")}
        actions={
          <Button onClick={() => setAddOpen(true)}>
            <Plus className="size-4" />
            {t("ssoDomains.addButton")}
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
              <p className="font-medium">{t("ssoDomains.loadError")}</p>
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
              <p className="font-medium">{t("ssoDomains.empty")}</p>
            </div>
            <Button onClick={() => setAddOpen(true)}>
              <Plus className="size-4" />
              {t("ssoDomains.addButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && domains.length > 0 && (
        <ul className="flex flex-col gap-2.5">
          {domains.map((domain) => {
            const verified = domain.status === "verified";
            return (
              <li
                key={domain.id}
                data-testid="sso-domain-card"
                className="card-hover flex flex-col gap-3 rounded-lg border border-border bg-card p-4 shadow-card"
              >
                <div className="flex items-center gap-3.5">
                  <div className="flex size-10 shrink-0 items-center justify-center rounded-[9px] bg-secondary">
                    <Globe className="size-[18px] text-muted-foreground" aria-hidden="true" />
                  </div>

                  <div className="min-w-0 flex-1">
                    <span className="font-mono text-[15px] font-medium text-strong">{domain.domain}</span>
                    <div className="mt-1 text-[12.5px] text-muted-foreground">{formatDateTime(domain.created)}</div>
                  </div>

                  <div className="flex shrink-0 items-center gap-2">
                    <span
                      aria-hidden="true"
                      data-testid="sso-domain-status-dot"
                      className={`size-2 rounded-full ${verified ? "bg-primary" : "bg-muted-foreground"}`}
                    />
                    <span className="text-[13px] text-foreground">
                      {verified ? t("ssoDomains.statusVerified") : t("ssoDomains.statusPending")}
                    </span>
                  </div>
                </div>

                <div className="flex flex-wrap items-center justify-end gap-1.5">
                  {domain.status === "pending" && (
                    <Button
                      variant="outline"
                      size="sm"
                      aria-label={t("ssoDomains.verifyAria", { domain: domain.domain })}
                      disabled={verifyingId === domain.id}
                      onClick={() => handleVerify(domain)}
                    >
                      {verifyingId === domain.id ? t("ssoDomains.verifying") : t("ssoDomains.verify")}
                    </Button>
                  )}
                  <Button
                    variant="ghost"
                    size="icon-sm"
                    aria-label={t("ssoDomains.removeAria", { domain: domain.domain })}
                    onClick={() => setRemovingDomain(domain)}
                  >
                    <Trash2 className="size-3.5" />
                  </Button>
                </div>

                {domain.status === "pending" && (
                  <div className="flex flex-col gap-2 rounded-lg border border-border bg-secondary/60 p-3.5 text-sm">
                    <p className="text-muted-foreground">
                      {t("ssoDomains.verifyInstructions", { domain: domain.domain })}
                    </p>
                    <div className="flex flex-col gap-1.5">
                      <div className="flex flex-wrap items-baseline gap-1.5">
                        <span className="text-xs text-muted-foreground">{t("ssoDomains.txtName")}:</span>
                        <span className="break-all rounded-md bg-secondary px-2 py-0.5 font-mono text-[12.5px]">
                          {domain.txt_name}
                        </span>
                      </div>
                      <div className="flex flex-wrap items-baseline gap-1.5">
                        <span className="text-xs text-muted-foreground">{t("ssoDomains.txtValue")}:</span>
                        <span className="break-all rounded-md bg-secondary px-2 py-0.5 font-mono text-[12.5px]">
                          {domain.txt_value}
                        </span>
                      </div>
                    </div>
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      )}

      <AddSsoDomainDialog open={addOpen} onOpenChange={setAddOpen} />

      <AlertDialog open={removingDomain != null} onOpenChange={(open) => !open && setRemovingDomain(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("ssoDomains.removeTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("ssoDomains.removeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteDomain.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction variant="destructive" disabled={deleteDomain.isPending} onClick={handleConfirmRemove}>
              {deleteDomain.isPending ? t("ssoDomains.removing") : t("ssoDomains.remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

interface FormErrors {
  domain?: string;
  form?: string;
}

interface AddSsoDomainDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function AddSsoDomainDialog({ open, onOpenChange }: AddSsoDomainDialogProps) {
  const t = useT();
  const [domain, setDomain] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const createDomain = useCreateSsoDomain();

  function reset() {
    setDomain("");
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    const trimmed = domain.trim();
    if (!trimmed) {
      next.domain = t("ssoDomains.domainRequired");
    } else if (!DOMAIN_RE.test(trimmed)) {
      next.domain = t("ssoDomains.createInvalidError");
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
      await createDomain.mutateAsync(domain.trim());
      toast.success(t("ssoDomains.createdSuccess"));
      reset();
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 409) {
        setErrors({ form: t("ssoDomains.createConflictError") });
      } else if (err instanceof ApiError && err.status === 400) {
        setErrors({ form: t("ssoDomains.createInvalidError") });
      } else if (err instanceof ApiError && err.status === 429) {
        setErrors({ form: t("common.rateLimited") });
      } else {
        setErrors({ form: t("ssoDomains.createGenericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("ssoDomains.addButton")}</DialogTitle>
            <DialogDescription>{t("ssoDomains.subtitle")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="add-sso-domain" className={FIELD_LABEL_CLASS}>
                {t("ssoDomains.domainLabel")}
              </Label>
              <Input
                id="add-sso-domain"
                type="text"
                placeholder={t("ssoDomains.domainPlaceholder")}
                value={domain}
                onChange={(e) => setDomain(e.target.value)}
                aria-invalid={errors.domain != null}
                autoFocus
              />
              {errors.domain && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.domain}
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
              {createDomain.isPending ? t("ssoDomains.adding") : t("ssoDomains.add")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
