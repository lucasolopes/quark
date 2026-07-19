import { useState, type FormEvent } from "react";
import { ArrowRight, Check, Copy, ExternalLink, Loader2 } from "lucide-react";
import { useNavigate } from "react-router-dom";
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
import { useT, type MessageKey } from "@/i18n";
import { ApiError, api } from "@/lib/api";
import { isHttpUrl } from "@/lib/codeguard";
import { isUnauthorized, mutationErrorToast } from "@/lib/mutation-error";
import { useCreatePixel, useCreateWebhook, useSheetsStatus, useSheetsSync, useSheetsDisconnect } from "@/lib/queries";
import { formatDateTime } from "@/lib/format";
import { WEBHOOK_EVENTS, type PixelProvider, type SubscriptionKind, type WebhookEvent } from "@/lib/types";

/**
 * Which real quark feature powers an integration. `sheets` is the one native
 * OAuth connector (its own connect/sync/disconnect card); `soon` is not built
 * yet.
 */
type PoweredBy = "webhooks" | "pixels" | "sheets" | "soon";

type Category = "notifications" | "automation" | "analytics" | "devData";

interface Integration {
  id: string;
  name: string;
  /** Short monogram shown inside the brand-colored badge (e.g. "Sl"). */
  mono: string;
  /** Brand color, used ONLY as the mono badge fill. */
  color: string;
  descKey: MessageKey;
  category: Category;
  poweredBy: PoweredBy;
}

/**
 * Curated catalog. Each item routes to the real quark feature that enables it.
 * quark has no per-integration OAuth connectors, so there are no connected
 * states here — a card either navigates to the enabling feature or, when the
 * integration is not built yet, is marked "coming soon" and is not clickable.
 */
const INTEGRATIONS: Integration[] = [
  // Notifications — powered by Webhooks.
  { id: "slack", name: "Slack", mono: "Sl", color: "#4A154B", descKey: "extensions.slackDesc", category: "notifications", poweredBy: "webhooks" },
  { id: "discord", name: "Discord", mono: "D", color: "#5865F2", descKey: "extensions.discordDesc", category: "notifications", poweredBy: "webhooks" },
  { id: "telegram", name: "Telegram", mono: "T", color: "#26A5E4", descKey: "extensions.telegramDesc", category: "notifications", poweredBy: "webhooks" },
  // Automation — powered by Webhooks.
  { id: "zapier", name: "Zapier", mono: "Z", color: "#FF4A00", descKey: "extensions.zapierDesc", category: "automation", poweredBy: "webhooks" },
  { id: "make", name: "Make", mono: "M", color: "#6D00CC", descKey: "extensions.makeDesc", category: "automation", poweredBy: "webhooks" },
  { id: "n8n", name: "n8n", mono: "n8", color: "#EA4B71", descKey: "extensions.n8nDesc", category: "automation", poweredBy: "webhooks" },
  { id: "sheets", name: "Google Sheets", mono: "GS", color: "#0F9D58", descKey: "extensions.sheetsDesc", category: "automation", poweredBy: "sheets" },
  // Analytics — GA4 and Meta powered by Pixels; the rest not built yet.
  { id: "ga4", name: "GA4 Measurement", mono: "GA", color: "#E37400", descKey: "extensions.ga4Desc", category: "analytics", poweredBy: "pixels" },
  { id: "meta", name: "Meta CAPI", mono: "f", color: "#0866FF", descKey: "extensions.metaDesc", category: "analytics", poweredBy: "pixels" },
  { id: "gtm", name: "Tag Manager", mono: "GTM", color: "#246FDB", descKey: "extensions.gtmDesc", category: "analytics", poweredBy: "soon" },
  { id: "tiktok", name: "TikTok Events", mono: "TT", color: "#111318", descKey: "extensions.tiktokDesc", category: "analytics", poweredBy: "soon" },
  { id: "linkedin", name: "LinkedIn CAPI", mono: "in", color: "#0A66C2", descKey: "extensions.linkedinDesc", category: "analytics", poweredBy: "soon" },
  // Dev & Data — not built yet.
  { id: "notion", name: "Notion", mono: "N", color: "#111318", descKey: "extensions.notionDesc", category: "devData", poweredBy: "soon" },
];

/** Render order of the category groups, with their eyebrow label keys. */
const CATEGORY_ORDER: { category: Category; labelKey: MessageKey }[] = [
  { category: "automation", labelKey: "extensions.categoryAutomation" },
  { category: "notifications", labelKey: "extensions.categoryNotifications" },
  { category: "analytics", labelKey: "extensions.categoryAnalytics" },
  { category: "devData", labelKey: "extensions.categoryDevData" },
];

/** Maps each webhook event to its i18n label key (reused from the Webhooks screen). */
const EVENT_LABEL_KEY: Record<WebhookEvent, MessageKey> = {
  "link.created": "webhooks.eventCreated",
  "link.updated": "webhooks.eventUpdated",
  "link.deleted": "webhooks.eventDeleted",
  "link.expired": "webhooks.eventExpired",
  "link.clicked": "webhooks.eventClicked",
  "link.threshold_reached": "webhooks.eventThresholdReached",
};

/**
 * Fixed webhook `kind` per integration id (aligned with LUC-15: no kind
 * selector in the UI). Native channels sign nothing and POST a channel-shaped
 * payload; the automation tools (Zapier/Make/n8n) are `generic` and get an
 * HMAC signing secret shown once.
 */
const WEBHOOK_KIND_BY_ID: Record<string, SubscriptionKind> = {
  slack: "slack",
  discord: "discord",
  telegram: "telegram",
  zapier: "generic",
  make: "generic",
  n8n: "generic",
};

/** Fixed pixel provider per integration id. */
const PIXEL_PROVIDER_BY_ID: Record<string, PixelProvider> = {
  ga4: "ga4",
  meta: "meta_capi",
};

export function Extensions() {
  const t = useT();

  return (
    <div className="flex flex-col gap-6">
      <div>
        <h1 className="font-heading text-2xl font-semibold">{t("extensions.heading")}</h1>
        <p className="mt-1 text-sm text-muted-foreground">{t("extensions.subtitle")}</p>
      </div>

      {CATEGORY_ORDER.map(({ category, labelKey }) => {
        const items = INTEGRATIONS.filter((i) => i.category === category);
        if (items.length === 0) return null;
        return (
          <section key={category} className="flex flex-col gap-3" aria-labelledby={`ext-group-${category}`}>
            <h2
              id={`ext-group-${category}`}
              className="font-mono text-[10px] font-medium tracking-[0.14em] text-muted-foreground uppercase"
            >
              {t(labelKey)}
            </h2>
            <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              {items.map((integration) => (
                <IntegrationCard key={integration.id} integration={integration} />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

function IntegrationCard({ integration }: { integration: Integration }) {
  const t = useT();
  const isSoon = integration.poweredBy === "soon";

  return (
    <Card className="flex flex-col justify-between gap-3 transition-[transform,border-color] duration-200 hover:-translate-y-[3px] hover:border-primary/30 motion-reduce:transition-none motion-reduce:hover:translate-y-0">
      <CardContent className="flex flex-col gap-3">
        <div className="flex items-start justify-between gap-2">
          <span
            aria-hidden="true"
            style={{ backgroundColor: integration.color }}
            className="flex size-10 shrink-0 items-center justify-center rounded-lg font-heading text-sm font-semibold text-white"
          >
            {integration.mono}
          </span>
          {isSoon && <Badge variant="secondary">{t("extensions.comingSoon")}</Badge>}
        </div>
        <div className="font-heading text-base font-medium">{integration.name}</div>
        <p className="text-sm text-muted-foreground">{t(integration.descKey)}</p>
      </CardContent>
      <CardContent>
        {integration.poweredBy === "webhooks" && <WebhookAction integration={integration} />}
        {integration.poweredBy === "pixels" && <PixelAction integration={integration} />}
        {integration.poweredBy === "sheets" && <SheetsAction />}
        {isSoon && (
          <Button variant="outline" size="sm" className="w-full" disabled>
            {t("extensions.comingSoon")}
          </Button>
        )}
      </CardContent>
    </Card>
  );
}

/**
 * The Google Sheets connector action. Driven by `useSheetsStatus`:
 * - connector off / unavailable (status endpoint 401/404): falls back to the
 *   old "via Webhooks" route, so the page never errors when the connector is not
 *   configured;
 * - not connected: a "Connect Google Sheets" button that fetches the consent URL
 *   (carrying the admin credential) and navigates the browser to Google;
 * - connected: the connected email, a link to the spreadsheet, the last-sync
 *   time (and error detail if the last sync failed), a "Sync now" button, and a
 *   "Disconnect" button.
 */
function SheetsAction() {
  const t = useT();
  const navigate = useNavigate();
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
      <Button variant="outline" size="sm" className="w-full" disabled>
        <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
      </Button>
    );
  }

  // Connector off or unavailable: keep the pre-connector behavior (route to Webhooks).
  if (!data || data.unavailable) {
    return (
      <Button variant="outline" size="sm" className="w-full" onClick={() => navigate("/webhooks")}>
        {t("extensions.viaWebhooks")}
        <ArrowRight className="size-3.5" />
      </Button>
    );
  }

  if (!data.connected) {
    return (
      <Button variant="outline" size="sm" className="w-full" disabled={connecting} onClick={handleConnect}>
        {connecting && <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />}
        {t("extensions.sheetsConnect")}
      </Button>
    );
  }

  const syncError = data.last_status.state === "error" ? data.last_status.detail : undefined;

  return (
    <div className="flex flex-col gap-2">
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
        <Button variant="outline" size="sm" className="flex-1" disabled={sync.isPending} onClick={handleSync}>
          {sync.isPending && <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />}
          {sync.isPending ? t("extensions.sheetsSyncing") : t("extensions.sheetsSyncNow")}
        </Button>
        <Button variant="ghost" size="sm" disabled={disconnect.isPending} onClick={handleDisconnect}>
          {t("extensions.sheetsDisconnect")}
        </Button>
      </div>
    </div>
  );
}

/**
 * Inline webhook activation for a notifications/automation card. Opens a
 * compact dialog (URL + events, all events selected by default) and creates
 * the subscription with the integration's fixed `kind` via `useCreateWebhook`.
 * For `generic` (Zapier/Make/n8n) it reveals the signing secret once, matching
 * the Webhooks screen; native channels get a success toast. No kind selector is
 * ever shown (LUC-15).
 */
function WebhookAction({ integration }: { integration: Integration }) {
  const t = useT();
  const navigate = useNavigate();
  const kind = WEBHOOK_KIND_BY_ID[integration.id] ?? "generic";
  const isChannel = kind !== "generic";

  const [open, setOpen] = useState(false);
  const [url, setUrl] = useState("");
  const [events, setEvents] = useState<WebhookEvent[]>([...WEBHOOK_EVENTS]);
  const [errors, setErrors] = useState<{ url?: string; events?: string; form?: string }>({});
  const [createdSecret, setCreatedSecret] = useState<string | null>(null);
  const [justCopiedSecret, setJustCopiedSecret] = useState(false);
  const createWebhook = useCreateWebhook();

  function reset() {
    setUrl("");
    setEvents([...WEBHOOK_EVENTS]);
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    setOpen(next);
  }

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
      const result = await createWebhook.mutateAsync({ url: url.trim(), events, active: true, kind });
      reset();
      setOpen(false);
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

  return (
    <div className="flex flex-col gap-2">
      <Button variant="outline" size="sm" className="w-full" onClick={() => setOpen(true)}>
        {t("extensions.activate")}
      </Button>
      <button
        type="button"
        className="text-xs text-muted-foreground underline-offset-4 hover:underline"
        onClick={() => navigate("/webhooks")}
      >
        {t("extensions.manageInWebhooks")}
      </button>

      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent>
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>{t("extensions.activateTitle", { name: integration.name })}</DialogTitle>
              <DialogDescription>{t("extensions.webhookModalDescription")}</DialogDescription>
            </DialogHeader>

            <div className="flex flex-col gap-3 py-3">
              <div className="flex flex-col gap-1.5">
                <Label htmlFor={`ext-webhook-url-${integration.id}`}>
                  {isChannel ? t("extensions.webhookUrlLabelChannel") : t("extensions.webhookUrlLabelEndpoint")}
                </Label>
                <Input
                  id={`ext-webhook-url-${integration.id}`}
                  type="text"
                  placeholder={
                    isChannel
                      ? t("extensions.webhookUrlPlaceholderChannel")
                      : t("extensions.webhookUrlPlaceholderEndpoint")
                  }
                  value={url}
                  onChange={(e) => setUrl(e.target.value)}
                  aria-invalid={errors.url != null}
                  autoFocus
                />
                {errors.url && (
                  <p className="text-sm text-destructive" role="alert">
                    {errors.url}
                  </p>
                )}
              </div>

              {!isChannel && (
                <p className="text-sm text-muted-foreground">{t("webhooks.create.secretNoticeGeneric")}</p>
              )}

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
 * Inline pixel activation for a GA4/Meta analytics card. Opens a compact
 * dialog with the provider's two credential fields (fixed provider, no
 * selector) and creates the pixel via `useCreatePixel`.
 */
function PixelAction({ integration }: { integration: Integration }) {
  const t = useT();
  const navigate = useNavigate();
  const provider = PIXEL_PROVIDER_BY_ID[integration.id] ?? "ga4";
  const isGa4 = provider === "ga4";

  const [open, setOpen] = useState(false);
  const [measurementId, setMeasurementId] = useState("");
  const [apiSecret, setApiSecret] = useState("");
  const [pixelId, setPixelId] = useState("");
  const [accessToken, setAccessToken] = useState("");
  const [errors, setErrors] = useState<{ a?: boolean; b?: boolean; form?: string }>({});
  const createPixel = useCreatePixel();

  function reset() {
    setMeasurementId("");
    setApiSecret("");
    setPixelId("");
    setAccessToken("");
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    setOpen(next);
  }

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
      reset();
      setOpen(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) toast.error(t("common.rateLimited"));
      else setErrors({ form: t("pixels.dialog.genericError") });
    }
  }

  const firstId = `ext-pixel-first-${integration.id}`;
  const secondId = `ext-pixel-second-${integration.id}`;

  return (
    <div className="flex flex-col gap-2">
      <Button variant="outline" size="sm" className="w-full" onClick={() => setOpen(true)}>
        {t("extensions.activate")}
      </Button>
      <button
        type="button"
        className="text-xs text-muted-foreground underline-offset-4 hover:underline"
        onClick={() => navigate("/pixels")}
      >
        {t("extensions.manageInPixels")}
      </button>

      <Dialog open={open} onOpenChange={handleOpenChange}>
        <DialogContent>
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>{t("extensions.activateTitle", { name: integration.name })}</DialogTitle>
              <DialogDescription>{t("pixels.dialog.description")}</DialogDescription>
            </DialogHeader>

            <div className="flex flex-col gap-3 py-3">
              <div className="flex flex-col gap-1.5">
                <label htmlFor={firstId} className="text-sm font-medium">
                  {isGa4 ? t("pixels.dialog.measurementIdLabel") : t("pixels.dialog.pixelIdLabel")}
                </label>
                <Input
                  id={firstId}
                  type="text"
                  placeholder={
                    isGa4 ? t("pixels.dialog.measurementIdPlaceholder") : t("pixels.dialog.pixelIdPlaceholder")
                  }
                  value={isGa4 ? measurementId : pixelId}
                  onChange={(e) => (isGa4 ? setMeasurementId(e.target.value) : setPixelId(e.target.value))}
                  aria-invalid={errors.a === true}
                  autoFocus
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
                  placeholder={
                    isGa4 ? t("pixels.dialog.apiSecretPlaceholder") : t("pixels.dialog.accessTokenPlaceholder")
                  }
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
            </div>

            <DialogFooter>
              <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
                {t("common.cancel")}
              </Button>
              <Button type="submit" disabled={createPixel.isPending}>
                {createPixel.isPending ? t("pixels.dialog.submitting") : t("pixels.dialog.submit")}
              </Button>
            </DialogFooter>
          </form>
        </DialogContent>
      </Dialog>
    </div>
  );
}
