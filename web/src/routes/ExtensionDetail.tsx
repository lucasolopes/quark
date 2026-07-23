import { useState, type FormEvent } from "react";
import { ArrowLeft, Check, CheckCircle2, Copy, ExternalLink, Loader2 } from "lucide-react";
import { Link, Navigate, useNavigate, useParams } from "react-router-dom";
import { toast } from "sonner";
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
import { useT } from "@/i18n";
import { ApiError, api } from "@/lib/api";
import { isHttpUrl } from "@/lib/codeguard";
import {
  EVENT_LABEL_KEY,
  PIXEL_PROVIDER_BY_ID,
  WEBHOOK_KIND_BY_ID,
  getIntegration,
  useConnectedIds,
  type Integration,
} from "@/lib/connectors";
import { formatDateTime } from "@/lib/format";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useCreatePixel, useCreateWebhook, useDeleteWebhook, useMe, usePixels, useSheetsStatus, useSheetsSync, useSheetsDisconnect, useWebhooks } from "@/lib/queries";
import { WEBHOOK_EVENTS, type WebhookEvent } from "@/lib/types";

/**
 * Dedicated per-integration view (`/extensions/:id`). Renders a header (brand
 * badge, name, description, connection status) and the connector's own connect
 * surface: OAuth (Sheets), a webhook endpoint form, or a pixel-credentials
 * form. Unknown ids redirect back to the catalog.
 */
export function ExtensionDetail() {
  const t = useT();
  const { id } = useParams<{ id: string }>();
  const integration = getIntegration(id);
  const connected = useConnectedIds();

  if (!integration) return <Navigate to="/extensions" replace />;
  const isConnected = connected.has(integration.id);
  const isSoon = integration.poweredBy === "soon";

  return (
    <div className="flex flex-col gap-6">
      <div className="flex items-center gap-3">
        <Button
          variant="ghost"
          size="icon-sm"
          aria-label={t("extensions.backAria")}
          nativeButton={false}
          render={<Link to="/extensions" />}
        >
          <ArrowLeft className="size-4" />
        </Button>
      </div>

      <div className="flex items-start gap-4">
        <span
          aria-hidden="true"
          style={{ backgroundColor: integration.color }}
          className="flex size-12 shrink-0 items-center justify-center rounded-xl font-heading text-base font-semibold text-white"
        >
          {integration.mono}
        </span>
        <div className="flex flex-col gap-1">
          <div className="flex items-center gap-2">
            <h1 className="font-heading text-2xl font-semibold">{integration.name}</h1>
            {isConnected ? (
              <Badge>
                <CheckCircle2 className="size-3" aria-hidden="true" />
                {t("extensions.connected")}
              </Badge>
            ) : isSoon ? (
              <Badge variant="secondary">{t("extensions.comingSoon")}</Badge>
            ) : (
              <Badge variant="secondary">{t("extensions.notConnected")}</Badge>
            )}
          </div>
          <p className="max-w-prose text-sm text-muted-foreground">{t(integration.descKey)}</p>
        </div>
      </div>

      {integration.poweredBy === "webhooks" && <WebhookPanel integration={integration} />}
      {integration.poweredBy === "pixels" && <PixelPanel integration={integration} />}
      {integration.poweredBy === "sheets" && <SheetsPanel />}
      {isSoon && <SoonPanel />}
    </div>
  );
}

/** Read-only panel for integrations that are not built yet. */
function SoonPanel() {
  const t = useT();
  return (
    <Card>
      <CardContent className="flex flex-col gap-1 py-6 text-center">
        <p className="text-sm font-medium">{t("extensions.comingSoon")}</p>
        <p className="text-sm text-muted-foreground">{t("extensions.soonDetail")}</p>
      </CardContent>
    </Card>
  );
}

/**
 * The Google Sheets connector. Driven by `useSheetsStatus`:
 * - connector off / unavailable (status endpoint 401/404): shows an
 *   "unavailable on this instance" notice (no misleading route to Webhooks);
 * - not connected: a "Connect Google Sheets" button that fetches the consent
 *   URL (carrying the admin credential) and navigates the browser to Google;
 * - connected: the connected email, a link to the spreadsheet, the last-sync
 *   time (and error detail if the last sync failed), plus Sync now / Disconnect.
 */
function SheetsPanel() {
  const t = useT();
  const { data, isLoading } = useSheetsStatus();
  const sync = useSheetsSync();
  const disconnect = useSheetsDisconnect();
  const [connecting, setConnecting] = useState(false);

  async function handleConnect() {
    setConnecting(true);
    try {
      const { url } = await api.sheetsConnect();
      window.location.href = url;
    } catch (err) {
      setConnecting(false);
      mutationErrorToast(err, () => t("extensions.sheetsConnectError"));
    }
  }

  async function handleSync() {
    try {
      const status = await sync.mutateAsync();
      if (status.last_status.state === "error") {
        toast.error(t("extensions.sheetsSyncErrorToast"));
      } else {
        toast.success(t("extensions.sheetsSyncSuccessToast"));
      }
    } catch (err) {
      mutationErrorToast(err, () => t("extensions.sheetsSyncErrorToast"));
    }
  }

  async function handleDisconnect() {
    try {
      await disconnect.mutateAsync();
      toast.success(t("extensions.sheetsDisconnectToast"));
    } catch (err) {
      mutationErrorToast(err, () => t("extensions.sheetsDisconnectError"));
    }
  }

  if (isLoading) {
    return (
      <Card>
        <CardContent className="flex justify-center py-6">
          <Loader2 className="size-5 animate-spin text-muted-foreground" aria-hidden="true" />
        </CardContent>
      </Card>
    );
  }

  // Connector not configured on this instance: say so plainly instead of
  // routing to the generic Webhooks screen (LUC-87: kill the confusing fallback).
  if (!data || data.unavailable) {
    return (
      <Card>
        <CardContent className="py-6 text-sm text-muted-foreground">{t("extensions.unavailable")}</CardContent>
      </Card>
    );
  }

  if (!data.connected) {
    return (
      <Card>
        <CardContent className="flex flex-col items-start gap-3 py-6">
          <p className="text-sm text-muted-foreground">{t("extensions.sheetsConnectPrompt")}</p>
          <Button disabled={connecting} onClick={handleConnect}>
            {connecting && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
            {t("extensions.sheetsConnect")}
          </Button>
        </CardContent>
      </Card>
    );
  }

  const syncError = data.last_status.state === "error" ? data.last_status.detail : undefined;

  return (
    <Card>
      <CardContent className="flex flex-col gap-3 py-6">
        {data.email && <p className="text-sm text-muted-foreground">{t("extensions.sheetsConnectedAs", { email: data.email })}</p>}
        <p className="text-xs text-muted-foreground">
          {data.last_sync
            ? t("extensions.sheetsLastSync", { time: formatDateTime(data.last_sync) })
            : t("extensions.sheetsNeverSynced")}
        </p>
        {syncError && <p className="text-xs text-destructive">{t("extensions.sheetsSyncError", { detail: syncError })}</p>}
        {data.spreadsheet_url && (
          <a
            href={data.spreadsheet_url}
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-1.5 text-sm text-primary underline-offset-4 hover:underline"
          >
            {t("extensions.sheetsOpenSheet")}
            <ExternalLink className="size-3.5" />
          </a>
        )}
        <div className="mt-1 flex gap-2">
          <Button variant="outline" size="sm" disabled={sync.isPending} onClick={handleSync}>
            {sync.isPending && <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />}
            {sync.isPending ? t("extensions.sheetsSyncing") : t("extensions.sheetsSyncNow")}
          </Button>
          <Button variant="ghost" size="sm" disabled={disconnect.isPending} onClick={handleDisconnect}>
            {t("extensions.sheetsDisconnect")}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}

/**
 * Webhook connect form for a notifications/automation integration. Renders the
 * destination URL + event selection inline on the page and creates the
 * subscription with the integration's fixed `kind` via `useCreateWebhook`. For
 * `generic` (Zapier/Make/n8n) it reveals the signing secret once; native
 * channels get a success toast. No kind selector is ever shown (LUC-15).
 */
function WebhookPanel({ integration }: { integration: Integration }) {
  const t = useT();
  const navigate = useNavigate();
  const me = useMe();
  const webhooks = useWebhooks();
  const deleteWebhook = useDeleteWebhook();
  const kind = WEBHOOK_KIND_BY_ID[integration.id] ?? "generic";
  const isChannel = kind !== "generic";
  // Existing subscriptions that belong to THIS integration. Webhooks created
  // via this panel (fase 3) carry `connector_id`, so we match on it exactly.
  // Legacy webhooks (no `connector_id`) fall back to matching by `kind`, which
  // still can't tell Zapier/Make/n8n apart from each other (they share
  // `kind: "generic"`).
  const existing = (webhooks.data?.webhooks ?? []).filter((w) =>
    w.connector_id != null ? w.connector_id === integration.id : isChannel && w.kind === kind,
  );
  const connected = existing.length > 0;
  // Slack "Add to Slack": when the OAuth connector is configured on the server,
  // lead with a one-click install (Slack returns the webhook URL — zero fields).
  // The manual URL form stays below as a fallback for any channel.
  const slackOauth = integration.id === "slack" && me.data?.slack_connect === true;
  const [connectingSlack, setConnectingSlack] = useState(false);

  async function handleDisconnect(id: number) {
    try {
      await deleteWebhook.mutateAsync(id);
      toast.success(t("extensions.channelDisconnectedToast"));
    } catch (err) {
      mutationErrorToast(err, () => t("extensions.channelDisconnectError"));
    }
  }

  /** A stable, non-secret label for a channel webhook: its host (the token in the
   * path is elided so it is never shown in full). */
  function webhookLabel(rawUrl: string): string {
    try {
      return new URL(rawUrl).host + "/…";
    } catch {
      return "…";
    }
  }

  async function handleAddToSlack() {
    setConnectingSlack(true);
    try {
      const { url } = await api.slackConnect();
      window.location.href = url;
    } catch (err) {
      setConnectingSlack(false);
      mutationErrorToast(err, () => t("extensions.slackConnectError"));
    }
  }

  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<WebhookEvent[]>([...WEBHOOK_EVENTS]);
  const [errors, setErrors] = useState<{ url?: string; events?: string; form?: string }>({});
  const [createdSecret, setCreatedSecret] = useState<string | null>(null);
  const [justCopiedSecret, setJustCopiedSecret] = useState(false);
  const createWebhook = useCreateWebhook();

  function toggleEvent(event: WebhookEvent, checked: boolean) {
    setEvents((current) => (checked ? [...current, event] : current.filter((e) => e !== event)));
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    const next: typeof errors = {};
    if (!url.trim()) next.url = t("webhooks.create.urlRequired");
    else if (!isHttpUrl(url)) next.url = t("webhooks.create.urlInvalid");
    if (events.length === 0) next.events = t("webhooks.create.eventsRequired");
    if (Object.keys(next).length > 0) {
      setErrors(next);
      return;
    }
    setErrors({});
    try {
      const result = await createWebhook.mutateAsync({ url: url.trim(), events, active: true, kind, connector_id: integration.id });
      setUrl("");
      setEvents([...WEBHOOK_EVENTS]);
      if (isChannel) {
        toast.success(t("extensions.webhookChannelSuccessToast", { name: integration.name }));
      } else if (result.secret) {
        setCreatedSecret(result.secret);
      } else {
        toast.success(t("webhooks.create.successToast"));
      }
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) toast.error(t("common.rateLimited"));
      else setErrors({ form: t("webhooks.create.genericError") });
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

  const urlId = `ext-webhook-url-${integration.id}`;

  return (
    <div className="flex flex-col gap-3">
      {connected && (
        <Card>
          <CardContent className="flex flex-col gap-3 py-6">
            <p className="text-sm font-medium">{t("extensions.channelConnectedTitle", { name: integration.name })}</p>
            <ul className="flex flex-col gap-3">
              {existing.map((w) => {
                const health = w.last_delivery_status?.state ?? "never";
                const deliveryError = health === "error" ? w.last_delivery_status?.detail : undefined;
                return (
                  <li key={w.id} className="flex flex-col gap-1">
                    <div className="flex items-center justify-between gap-3 text-sm">
                      <span className={w.label ? "font-medium" : "font-mono text-muted-foreground"}>
                        {w.label || webhookLabel(w.url)}
                      </span>
                      <Button
                        variant="ghost"
                        size="sm"
                        disabled={deleteWebhook.isPending}
                        onClick={() => handleDisconnect(w.id)}
                      >
                        {t("extensions.channelDisconnect")}
                      </Button>
                    </div>
                    {health === "ok" && w.last_delivery_at && (
                      <p className="text-xs text-muted-foreground">
                        {t("extensions.webhookLastDelivery", { time: formatDateTime(w.last_delivery_at) })}
                      </p>
                    )}
                    {deliveryError && (
                      <p className="text-xs text-destructive">{t("extensions.webhookDeliveryError", { detail: deliveryError })}</p>
                    )}
                  </li>
                );
              })}
            </ul>
          </CardContent>
        </Card>
      )}
      {slackOauth && (
        <Card>
          <CardContent className="flex flex-col items-start gap-3 py-6">
            <p className="text-sm text-muted-foreground">{t("extensions.slackConnectPrompt")}</p>
            <Button
              variant={connected ? "outline" : "default"}
              disabled={connectingSlack}
              onClick={handleAddToSlack}
            >
              {connectingSlack && <Loader2 className="size-4 animate-spin" aria-hidden="true" />}
              {connected ? t("extensions.slackAddAnother") : t("extensions.slackAddToSlack")}
            </Button>
          </CardContent>
        </Card>
      )}
      <Card>
        <CardContent className="py-6">
          <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
            {(slackOauth || connected) && <p className="text-sm font-medium">{t("extensions.orManual")}</p>}
            <p className="text-sm text-muted-foreground">{t("extensions.webhookModalDescription")}</p>

            <div className="flex flex-col gap-1.5">
              <Label htmlFor={urlId}>
                {isChannel ? t("extensions.webhookUrlLabelChannel") : t("extensions.webhookUrlLabelEndpoint")}
              </Label>
              <Input
                id={urlId}
                type="text"
                placeholder={
                  isChannel ? t("extensions.webhookUrlPlaceholderChannel") : t("extensions.webhookUrlPlaceholderEndpoint")
                }
                value={url}
                onChange={(e) => setUrl(e.target.value)}
                aria-invalid={errors.url != null}
              />
              {errors.url && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.url}
                </p>
              )}
            </div>

            {!isChannel && <p className="text-sm text-muted-foreground">{t("webhooks.create.secretNoticeGeneric")}</p>}

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

            {errors.form && (
              <p className="text-sm text-destructive" role="alert">
                {errors.form}
              </p>
            )}

            <div className="flex items-center gap-3">
              <Button type="submit" disabled={createWebhook.isPending}>
                {createWebhook.isPending ? t("webhooks.create.submitting") : t("extensions.activate")}
              </Button>
              <button
                type="button"
                className="text-xs text-muted-foreground underline-offset-4 hover:underline"
                onClick={() => navigate("/webhooks")}
              >
                {t("extensions.manageInWebhooks")}
              </button>
            </div>
          </form>
        </CardContent>
      </Card>

      <Dialog open={createdSecret != null} onOpenChange={(next) => !next && setCreatedSecret(null)}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("webhooks.secret.title")}</DialogTitle>
            <DialogDescription>{t("webhooks.secret.description")}</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-1.5 py-3">
            <Label htmlFor={`ext-webhook-secret-${integration.id}`}>{t("webhooks.secret.label")}</Label>
            <div className="flex items-center gap-2">
              <Input
                id={`ext-webhook-secret-${integration.id}`}
                type="text"
                readOnly
                value={createdSecret ?? ""}
                className="font-mono"
              />
              <Button
                type="button"
                variant="outline"
                size="icon"
                aria-label={t("webhooks.secret.copyAria")}
                onClick={handleCopySecret}
              >
                {justCopiedSecret ? <Check className="size-4 text-brand-ink" /> : <Copy className="size-4" />}
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

/**
 * Pixel connect form for a GA4/Meta analytics integration. Renders the
 * provider's two credential fields inline (fixed provider, no selector) and
 * creates the pixel via `useCreatePixel`.
 */
function PixelPanel({ integration }: { integration: Integration }) {
  const t = useT();
  const navigate = useNavigate();
  const provider = PIXEL_PROVIDER_BY_ID[integration.id] ?? "ga4";
  const isGa4 = provider === "ga4";
  const pixels = usePixels();
  const existingPixel = (pixels.data?.pixels ?? []).find((p) => p.provider === provider);
  const forwardHealth = existingPixel?.last_forward_status?.state ?? "never";
  const forwardError = forwardHealth === "error" ? existingPixel?.last_forward_status?.detail : undefined;

  const [measurementId, setMeasurementId] = useState("");
  const [apiSecret, setApiSecret] = useState("");
  const [pixelId, setPixelId] = useState("");
  const [accessToken, setAccessToken] = useState("");
  const [errors, setErrors] = useState<{ a?: boolean; b?: boolean; form?: string }>({});
  const createPixel = useCreatePixel();

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    const next: typeof errors = {};
    if (isGa4) {
      if (!measurementId.trim()) next.a = true;
      if (!apiSecret.trim()) next.b = true;
    } else {
      if (!pixelId.trim()) next.a = true;
      if (!accessToken.trim()) next.b = true;
    }
    if (next.a || next.b) {
      setErrors(next);
      return;
    }
    setErrors({});
    try {
      await createPixel.mutateAsync({
        provider,
        credentials: isGa4
          ? { measurement_id: measurementId.trim(), api_secret: apiSecret.trim() }
          : { pixel_id: pixelId.trim(), access_token: accessToken.trim() },
      });
      toast.success(t("pixels.dialog.successToast"));
      setMeasurementId("");
      setApiSecret("");
      setPixelId("");
      setAccessToken("");
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) toast.error(t("common.rateLimited"));
      else setErrors({ form: t("pixels.dialog.genericError") });
    }
  }

  const firstId = `ext-pixel-first-${integration.id}`;
  const secondId = `ext-pixel-second-${integration.id}`;

  return (
    <div className="flex flex-col gap-3">
      {existingPixel && (
        <Card>
          <CardContent className="flex flex-col gap-1 py-6">
            {forwardHealth === "ok" && existingPixel.last_forward_at && (
              <p className="text-xs text-muted-foreground">
                {t("extensions.pixelLastForward", { time: formatDateTime(existingPixel.last_forward_at) })}
              </p>
            )}
            {forwardError && <p className="text-xs text-destructive">{t("extensions.pixelForwardError", { detail: forwardError })}</p>}
          </CardContent>
        </Card>
      )}
      <Card>
        <CardContent className="py-6">
          <form className="flex flex-col gap-4" onSubmit={handleSubmit}>
            <p className="text-sm text-muted-foreground">{t("pixels.dialog.description")}</p>

            <div className="flex flex-col gap-1.5">
              <label htmlFor={firstId} className="text-sm font-medium">
                {isGa4 ? t("pixels.dialog.measurementIdLabel") : t("pixels.dialog.pixelIdLabel")}
              </label>
              <Input
                id={firstId}
                type="text"
                placeholder={isGa4 ? t("pixels.dialog.measurementIdPlaceholder") : t("pixels.dialog.pixelIdPlaceholder")}
                value={isGa4 ? measurementId : pixelId}
                onChange={(e) => (isGa4 ? setMeasurementId(e.target.value) : setPixelId(e.target.value))}
                aria-invalid={errors.a === true}
              />
              {errors.a && (
                <p className="text-sm text-destructive" role="alert">
                  {t("pixels.dialog.requiredField")}
                </p>
              )}
            </div>
            <div className="flex flex-col gap-1.5">
              <label htmlFor={secondId} className="text-sm font-medium">
                {isGa4 ? t("pixels.dialog.apiSecretLabel") : t("pixels.dialog.accessTokenLabel")}
              </label>
              <Input
                id={secondId}
                type="password"
                placeholder={isGa4 ? t("pixels.dialog.apiSecretPlaceholder") : t("pixels.dialog.accessTokenPlaceholder")}
                value={isGa4 ? apiSecret : accessToken}
                onChange={(e) => (isGa4 ? setApiSecret(e.target.value) : setAccessToken(e.target.value))}
                aria-invalid={errors.b === true}
              />
              {errors.b && (
                <p className="text-sm text-destructive" role="alert">
                  {t("pixels.dialog.requiredField")}
                </p>
              )}
            </div>

            {errors.form && (
              <p className="text-sm text-destructive" role="alert">
                {errors.form}
              </p>
            )}

            <div className="flex items-center gap-3">
              <Button type="submit" disabled={createPixel.isPending}>
                {createPixel.isPending ? t("pixels.dialog.submitting") : t("extensions.activate")}
              </Button>
              <button
                type="button"
                className="text-xs text-muted-foreground underline-offset-4 hover:underline"
                onClick={() => navigate("/pixels")}
              >
                {t("extensions.manageInPixels")}
              </button>
            </div>
          </form>
        </CardContent>
      </Card>
    </div>
  );
}
