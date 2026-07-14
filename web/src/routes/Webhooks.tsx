import { AlertTriangle, Check, Copy, Plus, RotateCw, Send, Trash2, Webhook as WebhookIcon } from "lucide-react";
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
import { Checkbox } from "@/components/ui/checkbox";
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
import { Switch } from "@/components/ui/switch";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { formatDateTime } from "@/lib/format";
import { isHttpUrl } from "@/lib/codeguard";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useCreateWebhook, useDeleteWebhook, usePatchWebhook, useTestWebhook, useWebhooks } from "@/lib/queries";
import { WEBHOOK_EVENTS, WEBHOOK_KINDS, type SubscriptionKind, type Webhook, type WebhookEvent } from "@/lib/types";

/** Maps each event id to its i18n label key (`webhooks.eventCreated`, etc). */
const EVENT_LABEL_KEY: Record<WebhookEvent, MessageKey> = {
  "link.created": "webhooks.eventCreated",
  "link.updated": "webhooks.eventUpdated",
  "link.deleted": "webhooks.eventDeleted",
  "link.expired": "webhooks.eventExpired",
  "link.clicked": "webhooks.eventClicked",
};

/** Maps each subscription kind to its i18n label key, used both for the badge and the type select. */
const KIND_LABEL_KEY: Record<SubscriptionKind, MessageKey> = {
  generic: "webhooks.kindGeneric",
  slack: "webhooks.kindSlack",
  discord: "webhooks.kindDiscord",
  telegram: "webhooks.kindTelegram",
};

/** URL field label, adapted per kind so the operator knows exactly what to paste. */
const KIND_URL_LABEL_KEY: Record<SubscriptionKind, MessageKey> = {
  generic: "webhooks.create.urlLabelGeneric",
  slack: "webhooks.create.urlLabelSlack",
  discord: "webhooks.create.urlLabelDiscord",
  telegram: "webhooks.create.urlLabelTelegram",
};

const KIND_URL_PLACEHOLDER_KEY: Record<SubscriptionKind, MessageKey> = {
  generic: "webhooks.create.urlPlaceholderGeneric",
  slack: "webhooks.create.urlPlaceholderSlack",
  discord: "webhooks.create.urlPlaceholderDiscord",
  telegram: "webhooks.create.urlPlaceholderTelegram",
};

/** Extra hint shown under the URL field for channel kinds (where to find the URL). No hint for generic. */
const KIND_URL_HINT_KEY: Partial<Record<SubscriptionKind, MessageKey>> = {
  slack: "webhooks.create.urlHintSlack",
  discord: "webhooks.create.urlHintDiscord",
  telegram: "webhooks.create.urlHintTelegram",
};

interface FormErrors {
  url?: string;
  events?: string;
  form?: string;
}

export function Webhooks() {
  const t = useT();
  const [createOpen, setCreateOpen] = useState(false);
  const [deletingWebhook, setDeletingWebhook] = useState<Webhook | null>(null);
  const [testingId, setTestingId] = useState<number | null>(null);
  const [createdSecret, setCreatedSecret] = useState<string | null>(null);
  const [justCopiedSecret, setJustCopiedSecret] = useState(false);

  const query = useWebhooks();
  const patchWebhook = usePatchWebhook();
  const deleteWebhook = useDeleteWebhook();
  const testWebhook = useTestWebhook();

  const webhooks = query.data?.webhooks ?? [];

  async function handleToggleActive(webhook: Webhook) {
    try {
      await patchWebhook.mutateAsync({ id: webhook.id, body: { active: !webhook.active } });
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("webhooks.activeUpdateError"),
      );
    }
  }

  async function handleTest(webhook: Webhook) {
    setTestingId(webhook.id);
    try {
      const result = await testWebhook.mutateAsync(webhook.id);
      if (result.delivered) {
        toast.success(t("webhooks.testSuccessToast", { status: result.status }));
      } else {
        toast.error(t("webhooks.testFailureToast", { status: result.status }));
      }
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(t("webhooks.testErrorToast"));
    } finally {
      setTestingId(null);
    }
  }

  async function handleConfirmDelete() {
    if (!deletingWebhook) return;
    try {
      await deleteWebhook.mutateAsync(deletingWebhook.id);
      toast.success(t("webhooks.deleteSuccess"));
      setDeletingWebhook(null);
    } catch (err) {
      mutationErrorToast(err, (e) =>
        e instanceof ApiError && e.status === 429 ? t("common.rateLimited") : t("webhooks.deleteGenericError"),
      );
    }
  }

  async function handleCopySecret() {
    if (!createdSecret) return;
    try {
      await navigator.clipboard.writeText(createdSecret);
      toast.success(t("webhooks.secret.copied"));
      setJustCopiedSecret(true);
      setTimeout(() => setJustCopiedSecret(false), 1500);
    } catch {
      toast.error(t("webhooks.secret.copyFailed"));
    }
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="font-heading text-2xl font-semibold">{t("webhooks.heading")}</h1>
          <p className="mt-1 text-sm text-muted-foreground">{t("webhooks.subtitle")}</p>
        </div>
        <Button onClick={() => setCreateOpen(true)}>
          <Plus className="size-4" />
          {t("webhooks.addButton")}
        </Button>
      </div>

      {query.isPending && <WebhooksSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("webhooks.loadError")}</p>
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

      {!query.isPending && !query.isError && webhooks.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <WebhookIcon className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("webhooks.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("webhooks.emptySubtitle")}</p>
            </div>
            <Button onClick={() => setCreateOpen(true)}>
              <Plus className="size-4" />
              {t("webhooks.addButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && webhooks.length > 0 && (
        <Card className="py-0">
          <Table>
            <caption className="sr-only">{t("webhooks.heading")}</caption>
            <TableHeader>
              <TableRow>
                <TableHead>{t("webhooks.columnUrl")}</TableHead>
                <TableHead>{t("webhooks.columnType")}</TableHead>
                <TableHead>{t("webhooks.columnEvents")}</TableHead>
                <TableHead>{t("webhooks.columnActive")}</TableHead>
                <TableHead>{t("webhooks.columnCreated")}</TableHead>
                <TableHead>
                  <span className="sr-only">{t("webhooks.actionsSr")}</span>
                </TableHead>
              </TableRow>
            </TableHeader>
            <TableBody>
              {webhooks.map((webhook) => (
                <TableRow key={webhook.id}>
                  <TableCell>
                    <span className="block max-w-64 truncate font-mono text-sm" title={webhook.url}>
                      {webhook.url}
                    </span>
                  </TableCell>
                  <TableCell>
                    <Badge variant="outline">{t(KIND_LABEL_KEY[webhook.kind ?? "generic"])}</Badge>
                  </TableCell>
                  <TableCell>
                    <div className="flex max-w-72 flex-wrap gap-1">
                      {webhook.events.map((event) => (
                        <Badge key={event} variant="secondary">
                          {t(EVENT_LABEL_KEY[event])}
                        </Badge>
                      ))}
                    </div>
                  </TableCell>
                  <TableCell>
                    <Switch
                      checked={webhook.active}
                      onCheckedChange={() => handleToggleActive(webhook)}
                      disabled={patchWebhook.isPending}
                      aria-label={
                        webhook.active
                          ? t("webhooks.deactivateAria", { url: webhook.url })
                          : t("webhooks.activateAria", { url: webhook.url })
                      }
                    />
                  </TableCell>
                  <TableCell className="text-muted-foreground">{formatDateTime(webhook.created)}</TableCell>
                  <TableCell>
                    <div className="flex items-center justify-end gap-1">
                      <Button
                        variant="outline"
                        size="sm"
                        aria-label={t("webhooks.testButtonAria", { url: webhook.url })}
                        disabled={testingId === webhook.id}
                        onClick={() => handleTest(webhook)}
                      >
                        <Send className="size-3.5" />
                        {testingId === webhook.id ? t("webhooks.testing") : t("webhooks.testButton")}
                      </Button>
                      <Button
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("webhooks.deleteAria", { url: webhook.url })}
                        onClick={() => setDeletingWebhook(webhook)}
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

      <CreateWebhookDialog
        open={createOpen}
        onOpenChange={setCreateOpen}
        onCreated={(secret) => setCreatedSecret(secret)}
      />

      <AlertDialog open={deletingWebhook != null} onOpenChange={(open) => !open && setDeletingWebhook(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("webhooks.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("webhooks.deleteDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deleteWebhook.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deleteWebhook.isPending}
              onClick={handleConfirmDelete}
            >
              {deleteWebhook.isPending ? t("webhooks.deleting") : t("webhooks.delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <Dialog
        open={createdSecret != null}
        onOpenChange={(open) => {
          if (!open) setCreatedSecret(null);
        }}
      >
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("webhooks.secret.title")}</DialogTitle>
            <DialogDescription>{t("webhooks.secret.description")}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5 py-3">
            <Label htmlFor="webhook-secret">{t("webhooks.secret.label")}</Label>
            <div className="flex items-center gap-2">
              <Input id="webhook-secret" type="text" readOnly value={createdSecret ?? ""} className="font-mono" />
              <Button
                type="button"
                variant="outline"
                size="icon"
                aria-label={t("webhooks.secret.copyAria")}
                onClick={handleCopySecret}
              >
                {justCopiedSecret ? <Check className="size-4 text-primary" /> : <Copy className="size-4" />}
              </Button>
            </div>
          </div>
          <DialogFooter>
            <Button type="button" onClick={() => setCreatedSecret(null)}>
              {t("webhooks.secret.done")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </div>
  );
}

function WebhooksSkeleton() {
  return (
    <div className="flex flex-col gap-2" aria-hidden="true">
      {Array.from({ length: 4 }).map((_, i) => (
        <Skeleton key={i} className="h-10 w-full" />
      ))}
    </div>
  );
}

interface CreateWebhookDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Called with the raw secret right after a successful creation, before the dialog closes. */
  onCreated: (secret: string) => void;
}

/**
 * Webhook creation dialog. Validates client-side (http/https URL, at least
 * one event selected) before calling the API. On success, hands the raw
 * secret up to the parent — the API only ever returns it this once.
 */
function CreateWebhookDialog({ open, onOpenChange, onCreated }: CreateWebhookDialogProps) {
  const t = useT();
  const [kind, setKind] = useState<SubscriptionKind>("generic");
  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<WebhookEvent[]>([]);
  const [active, setActive] = useState(true);
  const [errors, setErrors] = useState<FormErrors>({});
  const createWebhook = useCreateWebhook();

  function reset() {
    setKind("generic");
    setUrl("");
    setEvents([]);
    setActive(true);
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function toggleEvent(event: WebhookEvent, checked: boolean) {
    setEvents((current) => (checked ? [...current, event] : current.filter((e) => e !== event)));
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = t("webhooks.create.urlRequired");
    } else if (!isHttpUrl(url)) {
      next.url = t("webhooks.create.urlInvalid");
    }
    if (events.length === 0) {
      next.events = t("webhooks.create.eventsRequired");
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
      const result = await createWebhook.mutateAsync({ url: url.trim(), events, active, kind });
      toast.success(t("webhooks.create.successToast"));
      reset();
      onOpenChange(false);
      if (result.secret) onCreated(result.secret);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("webhooks.create.genericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("webhooks.create.title")}</DialogTitle>
            <DialogDescription>{t("webhooks.create.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <Label htmlFor="create-webhook-kind">{t("webhooks.create.typeLabel")}</Label>
              <select
                id="create-webhook-kind"
                className="border-input bg-transparent flex h-9 w-full rounded-md border px-3 py-1 text-sm shadow-xs outline-none"
                value={kind}
                onChange={(e) => setKind(e.target.value as SubscriptionKind)}
              >
                {WEBHOOK_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {t(KIND_LABEL_KEY[k])}
                  </option>
                ))}
              </select>
            </div>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor="create-webhook-url">{t(KIND_URL_LABEL_KEY[kind])}</Label>
              <Input
                id="create-webhook-url"
                type="text"
                placeholder={t(KIND_URL_PLACEHOLDER_KEY[kind])}
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                aria-invalid={errors.url != null}
                autoFocus
              />
              {KIND_URL_HINT_KEY[kind] && (
                <p className="text-sm text-muted-foreground">{t(KIND_URL_HINT_KEY[kind]!)}</p>
              )}
              {errors.url && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.url}
                </p>
              )}
            </div>

            <p className="text-sm text-muted-foreground">
              {kind === "generic" ? t("webhooks.create.secretNoticeGeneric") : t("webhooks.create.secretNoticeChannel")}
            </p>

            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-medium">{t("webhooks.create.eventsLabel")}</span>
              <div className="flex flex-col gap-2">
                {WEBHOOK_EVENTS.map((event) => (
                  <label key={event} className="flex items-center gap-2 text-sm font-normal">
                    <Checkbox
                      checked={events.includes(event)}
                      onCheckedChange={(checked) => toggleEvent(event, checked === true)}
                    />
                    {t(EVENT_LABEL_KEY[event])}
                  </label>
                ))}
              </div>
              {errors.events && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.events}
                </p>
              )}
            </div>

            <label className="flex items-center gap-2 text-sm font-medium">
              <Switch checked={active} onCheckedChange={setActive} />
              {t("webhooks.create.activeLabel")}
            </label>

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
            <Button type="submit" disabled={createWebhook.isPending}>
              {createWebhook.isPending ? t("webhooks.create.submitting") : t("webhooks.create.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
