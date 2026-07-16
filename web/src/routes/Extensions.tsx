import { useState } from "react";
import { ArrowRight, ExternalLink, Loader2 } from "lucide-react";
import { useNavigate } from "react-router-dom";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { useT, type MessageKey } from "@/i18n";
import { api } from "@/lib/api";
import { useSheetsStatus, useSheetsSync, useSheetsDisconnect } from "@/lib/queries";
import { mutationErrorToast } from "@/lib/mutation-error";
import { formatDateTime } from "@/lib/format";

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
  const navigate = useNavigate();
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
        {integration.poweredBy === "webhooks" && (
          <Button variant="outline" size="sm" className="w-full" onClick={() => navigate("/webhooks")}>
            {t("extensions.viaWebhooks")}
            <ArrowRight className="size-3.5" />
          </Button>
        )}
        {integration.poweredBy === "pixels" && (
          <Button variant="outline" size="sm" className="w-full" onClick={() => navigate("/pixels")}>
            {t("extensions.viaPixels")}
            <ArrowRight className="size-3.5" />
          </Button>
        )}
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
