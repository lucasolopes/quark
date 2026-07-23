import { ArrowRight, CheckCircle2 } from "lucide-react";
import { Link } from "react-router-dom";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent } from "@/components/ui/card";
import { useT } from "@/i18n";
import { CATEGORY_ORDER, INTEGRATIONS, useConnectedIds, type Integration } from "@/lib/connectors";

/**
 * The integrations center catalog. Each connectable integration is a card that
 * links to its own dedicated view (`/extensions/:id`) where it is connected and
 * managed. Not-yet-built integrations render as read-only "coming soon" cards
 * (no link). The connection state per card is derived from the backing feature
 * APIs via `useConnectedIds`.
 */
export function Extensions() {
  const t = useT();
  const connected = useConnectedIds();

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
                <IntegrationCard
                  key={integration.id}
                  integration={integration}
                  connected={connected.has(integration.id)}
                />
              ))}
            </div>
          </section>
        );
      })}
    </div>
  );
}

/** Shared card face (mono badge, name, description, status). */
function CardFace({ integration, connected }: { integration: Integration; connected: boolean }) {
  const t = useT();
  const isSoon = integration.poweredBy === "soon";
  return (
    <CardContent className="flex flex-1 flex-col gap-3">
      <div className="flex items-start justify-between gap-2">
        <span
          aria-hidden="true"
          style={{ backgroundColor: integration.color }}
          className="flex size-10 shrink-0 items-center justify-center rounded-lg font-heading text-sm font-semibold text-white"
        >
          {integration.mono}
        </span>
        {connected ? (
          <Badge>
            <CheckCircle2 className="size-3" aria-hidden="true" />
            {t("extensions.connected")}
          </Badge>
        ) : (
          isSoon && <Badge variant="secondary">{t("extensions.comingSoon")}</Badge>
        )}
      </div>
      <div className="font-heading text-base font-medium">{integration.name}</div>
      <p className="text-sm text-muted-foreground">{t(integration.descKey)}</p>
    </CardContent>
  );
}

function IntegrationCard({ integration, connected }: { integration: Integration; connected: boolean }) {
  const t = useT();
  const isSoon = integration.poweredBy === "soon";

  // Coming-soon integrations are informational only: no dedicated view yet, so
  // the card is not a link.
  if (isSoon) {
    return (
      <Card className="flex flex-col opacity-70">
        <CardFace integration={integration} connected={connected} />
      </Card>
    );
  }

  return (
    <Link
      to={`/extensions/${integration.id}`}
      className="group rounded-xl focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      <Card className="flex h-full flex-col transition-[transform,border-color] duration-200 group-hover:-translate-y-[3px] group-hover:border-primary/30 motion-reduce:transition-none motion-reduce:group-hover:translate-y-0">
        <CardFace integration={integration} connected={connected} />
        <CardContent className="flex items-center gap-1.5 pt-0 text-sm font-medium text-primary">
          {connected ? t("extensions.manage") : t("extensions.setUp")}
          <ArrowRight className="size-3.5 transition-transform duration-200 group-hover:translate-x-0.5 motion-reduce:transition-none" />
        </CardContent>
      </Card>
    </Link>
  );
}
