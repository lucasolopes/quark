import { AlertTriangle, Check, RotateCw } from "lucide-react";
import { useEffect, useRef, useState, type RefObject } from "react";
import { useSearchParams } from "react-router-dom";
import { toast } from "sonner";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Skeleton } from "@/components/ui/skeleton";
import { PageHeader } from "@/components/PageHeader";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { formatNumber } from "@/lib/format";
import { useBillingCatalog, useMe, useOpenPortal, useStartCheckout } from "@/lib/queries";
import type { CatalogPlan } from "@/lib/types";

/** Commercial contact for the Custom plan, until the domain migrates off quarkus.com.br (LUC-147). */
const CONTACT_EMAIL = "contato@quarkus.com.br";

/** Billing cycles the catalog's `prices` carry (matches the backend's Stripe lookup keys). */
type Cycle = "monthly" | "yearly";
/** Settlement currencies a price can be shown in. */
type Currency = "usd" | "brl";

/** Plans with no purchase flow: Free (nothing to buy) and Custom (negotiated, contact sales). */
const UNSELLABLE_PLANS = new Set(["free", "custom"]);

const LIMIT_ROWS: Array<{ key: keyof CatalogPlan["limits"]; labelKey: MessageKey }> = [
  { key: "domains", labelKey: "billing.limits.domains" },
  { key: "members", labelKey: "billing.limits.members" },
  { key: "automation_per_month", labelKey: "billing.limits.automation" },
  { key: "tracked_clicks_per_month", labelKey: "billing.limits.clicks" },
  { key: "retention_days", labelKey: "billing.limits.retention" },
];

/** Formats a price in cents as `$4` / `R$ 19` (no trailing `.00` on whole amounts). */
function formatPrice(cents: number, currency: Currency): string {
  const amount = cents / 100;
  const formatted = Number.isInteger(amount) ? String(amount) : amount.toFixed(2);
  return currency === "brl" ? `R$ ${formatted}` : `$${formatted}`;
}

/** Plan ids (`free`, `starter`, …) are stable identifiers shared with the backend catalog; title-case is enough, no i18n needed for a proper noun. */
function planLabel(plan: string): string {
  return plan.charAt(0).toUpperCase() + plan.slice(1);
}

/** Feature flags (`webhooks`, `sso`, …) are likewise backend identifiers; title-case for display. */
function featureLabel(feature: string): string {
  return feature.charAt(0).toUpperCase() + feature.slice(1);
}

/** Screen for `settings/billing`: the 5-plan comparison grid, upgrade via Checkout, and the portal fallback for an active subscription. */
export function Billing() {
  const t = useT();
  const [searchParams, setSearchParams] = useSearchParams();
  const [cycle, setCycle] = useState<Cycle>("monthly");
  const [currencyChoice, setCurrencyChoice] = useState<Currency>("usd");
  const [hasActiveSubscription, setHasActiveSubscription] = useState(false);
  const [actingOnPlan, setActingOnPlan] = useState<string | null>(null);
  const highlightRef = useRef<HTMLDivElement | null>(null);
  const subscriptionInitialized = useRef(false);
  /** The `highlight` value already scrolled to, or `null` before the first scroll. Storing the
   * value (not a boolean) means a later navigation to a *different* `highlight` — e.g. a second
   * 402 toast sending the user to `?highlight=B` while they're already on this screen, which
   * doesn't unmount the component — is recognized as new and scrolls again, while a re-render
   * with the same `highlight` (a catalog refetch, for instance) still only scrolls once. */
  const scrolledToHighlightRef = useRef<string | null>(null);

  const catalogQuery = useBillingCatalog();
  const me = useMe();
  const startCheckout = useStartCheckout();
  const openPortal = useOpenPortal();

  const catalog = catalogQuery.data;
  const isOwner = me.data?.memberships?.find((m) => m.tenant_id === me.data?.current_tenant)?.role === "owner";
  const lockedCurrency = (catalog?.currency_locked as Currency | null) ?? null;
  const currency = lockedCurrency ?? currencyChoice;
  const highlight = searchParams.get("highlight");
  const checkoutState = searchParams.get("checkout");

  // The webhook that flips `current_plan` after a successful Checkout can land a few
  // seconds after Stripe redirects back here, so this re-fetches the catalog a handful
  // of times instead of trusting the first response. There is no dedicated `/admin/plan`
  // client on the panel; `current_plan` rides in the same catalog payload, so refetching
  // the catalog does the same job without a second endpoint.
  useEffect(() => {
    if (!checkoutState) return;
    let interval: ReturnType<typeof setInterval> | undefined;
    if (checkoutState === "success") {
      toast.success(t("billing.checkoutSuccess"));
      let attempts = 0;
      interval = setInterval(() => {
        attempts += 1;
        void catalogQuery.refetch();
        if (attempts >= 3 && interval) clearInterval(interval);
      }, 2000);
    } else if (checkoutState === "cancel") {
      toast(t("billing.checkoutCanceled"));
    }
    const next = new URLSearchParams(searchParams);
    next.delete("checkout");
    setSearchParams(next, { replace: true });
    // Runs once on mount: the toast/refetch fires exactly once for the redirect that
    // brought the user here, then the param is stripped so a later re-render never repeats it.
    // The interval must still be torn down on unmount (navigating away mid-poll, or the test
    // harness tearing the component down) — otherwise it keeps firing `catalogQuery.refetch()`
    // against a component that's gone.
    return () => {
      if (interval) clearInterval(interval);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    if (!highlight || scrolledToHighlightRef.current === highlight) return;
    if (!highlightRef.current) return; // the catalog (and its cards) hasn't rendered yet; retried below once it does
    highlightRef.current.scrollIntoView?.({ behavior: "smooth", block: "center" });
    scrolledToHighlightRef.current = highlight;
    // Depends on `catalogQuery.isSuccess`, not `catalog` itself: `catalog` gets a new object
    // identity on every refetch (including the 3 checkout-success refetches that fire while
    // `highlight` is also set), which would re-run this effect on every one of them. `isSuccess`
    // flips once, from false to true, on the first successful load and then stays put — exactly the
    // "the highlighted card just mounted" signal this needs. `scrolledToHighlightRef` storing the
    // already-scrolled *value* (not a boolean) is what still caps it to one scroll per distinct
    // `highlight`, while letting a later `highlight` change scroll again.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [highlight, catalogQuery.isSuccess]);

  // Seeds `hasActiveSubscription` from the catalog the first time it loads: a workspace
  // already on a paid plan (`current_plan !== "free"`) almost certainly has an active Stripe
  // subscription, so its owner should land straight on "Manage in portal" instead of paying the
  // round-trip through a checkout attempt that would just come back 409. Runs once (guarded by
  // `subscriptionInitialized`) so it doesn't fight the 409 handler below, which is still the
  // source of truth if this guess and the real Stripe state ever diverge (e.g. the catalog is
  // momentarily stale right after a cancellation).
  useEffect(() => {
    if (!catalog || subscriptionInitialized.current) return;
    subscriptionInitialized.current = true;
    if (catalog.current_plan !== "free") setHasActiveSubscription(true);
  }, [catalog]);

  async function handlePlanAction(plan: string) {
    setActingOnPlan(plan);
    try {
      if (hasActiveSubscription) {
        const { url } = await openPortal.mutateAsync();
        window.location.assign(url);
        return;
      }
      try {
        const { url } = await startCheckout.mutateAsync({ plan, cycle, currency });
        window.location.assign(url);
      } catch (err) {
        if (err instanceof ApiError && err.status === 409) {
          setHasActiveSubscription(true);
          const { url } = await openPortal.mutateAsync();
          window.location.assign(url);
        } else {
          toast.error(t("common.retryHint"));
        }
      }
    } catch {
      toast.error(t("common.retryHint"));
    } finally {
      setActingOnPlan(null);
    }
  }

  return (
    <div className="flex flex-col gap-4 animate-rise">
      <PageHeader title={t("billing.title")} subtitle={t("billing.subtitle")} />

      {catalogQuery.isPending && <BillingSkeleton />}

      {catalogQuery.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <p className="font-medium">
              {catalogQuery.error instanceof Error ? catalogQuery.error.message : t("common.retryHint")}
            </p>
            <Button variant="outline" onClick={() => catalogQuery.refetch()}>
              <RotateCw className="size-4" />
              {t("common.retry")}
            </Button>
          </CardContent>
        </Card>
      )}

      {catalog && (
        <>
          <div className="flex flex-wrap items-center gap-3">
            <div className="inline-flex items-center gap-1 rounded-lg border border-border bg-card p-1">
              <Button
                type="button"
                size="sm"
                variant={cycle === "monthly" ? "default" : "ghost"}
                onClick={() => setCycle("monthly")}
              >
                {t("billing.monthly")}
              </Button>
              <Button
                type="button"
                size="sm"
                variant={cycle === "yearly" ? "default" : "ghost"}
                onClick={() => setCycle("yearly")}
              >
                {t("billing.yearly")}
              </Button>
            </div>
            {cycle === "yearly" && <Badge variant="secondary">{t("billing.yearlyBadge")}</Badge>}

            {lockedCurrency == null && (
              <div className="inline-flex items-center gap-1 rounded-lg border border-border bg-card p-1">
                <Button
                  type="button"
                  size="sm"
                  variant={currency === "usd" ? "default" : "ghost"}
                  onClick={() => setCurrencyChoice("usd")}
                >
                  USD
                </Button>
                <Button
                  type="button"
                  size="sm"
                  variant={currency === "brl" ? "default" : "ghost"}
                  onClick={() => setCurrencyChoice("brl")}
                >
                  BRL
                </Button>
              </div>
            )}
          </div>

          {!catalog.prices_available && <p className="text-sm text-muted-foreground">{t("billing.notConfigured")}</p>}

          <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-5">
            {catalog.plans.map((plan) => (
              <PlanCard
                key={plan.plan}
                plan={plan}
                isCurrent={plan.plan === catalog.current_plan}
                highlightRef={plan.plan === highlight ? highlightRef : undefined}
                isHighlighted={plan.plan === highlight}
                cycle={cycle}
                currency={currency}
                pricesAvailable={catalog.prices_available}
                isOwner={Boolean(isOwner)}
                hasActiveSubscription={hasActiveSubscription}
                pending={actingOnPlan === plan.plan}
                onAction={() => handlePlanAction(plan.plan)}
              />
            ))}
          </div>
        </>
      )}
    </div>
  );
}

interface PlanCardProps {
  plan: CatalogPlan;
  isCurrent: boolean;
  isHighlighted: boolean;
  highlightRef?: RefObject<HTMLDivElement | null>;
  cycle: Cycle;
  currency: Currency;
  pricesAvailable: boolean;
  isOwner: boolean;
  hasActiveSubscription: boolean;
  pending: boolean;
  onAction: () => void;
}

function PlanCard({
  plan,
  isCurrent,
  isHighlighted,
  highlightRef,
  cycle,
  currency,
  pricesAvailable,
  isOwner,
  hasActiveSubscription,
  pending,
  onAction,
}: PlanCardProps) {
  const t = useT();
  const isCustom = plan.plan === "custom";
  const sellable = pricesAvailable && !UNSELLABLE_PLANS.has(plan.plan) && plan.prices != null;
  const accented = isCurrent || isHighlighted;
  const price = plan.prices ? (cycle === "monthly" ? plan.prices.monthly : plan.prices.yearly) : null;
  const priceCents = price ? (currency === "brl" ? price.brl_cents : price.usd_cents) : 0;

  return (
    <Card
      ref={highlightRef}
      data-testid="plan-card"
      className={`card-hover flex flex-col ${accented ? "border-accent-line" : ""}`}
    >
      <CardContent className="flex flex-1 flex-col gap-4">
        <div className="flex items-center justify-between gap-2">
          <h3 className="font-heading text-lg font-semibold text-strong">{planLabel(plan.plan)}</h3>
          {isCurrent && <Badge>{t("billing.currentPlan")}</Badge>}
        </div>

        <div className="text-stat font-heading font-bold text-strong">
          {isCustom ? (
            <span className="text-lg">{t("billing.custom")}</span>
          ) : (
            formatPrice(priceCents, currency)
          )}
        </div>

        <ul className="flex flex-col gap-1.5 text-sm text-muted-foreground">
          {LIMIT_ROWS.map((row) => (
            <li key={row.key} className="flex items-center justify-between gap-2">
              <span>{t(row.labelKey)}</span>
              <span className="text-foreground">
                {plan.limits[row.key] == null ? t("billing.unlimited") : formatNumber(plan.limits[row.key] as number)}
              </span>
            </li>
          ))}
        </ul>

        {plan.features.length > 0 && (
          <ul className="flex flex-col gap-1.5 text-sm">
            {plan.features.map((feature) => (
              <li key={feature} className="flex items-center gap-1.5">
                <Check className="size-3.5 text-primary" aria-hidden="true" />
                {featureLabel(feature)}
              </li>
            ))}
          </ul>
        )}

        <div className="mt-auto pt-1">
          {isCustom ? (
            <Button variant="outline" render={<a href={`mailto:${CONTACT_EMAIL}`} />}>
              {t("billing.contactUs")}
            </Button>
          ) : sellable ? (
            isOwner ? (
              <Button onClick={onAction} disabled={pending}>
                {hasActiveSubscription ? t("billing.managePortal") : t("billing.upgrade")}
              </Button>
            ) : (
              <Button disabled title={t("billing.ownerOnly")}>
                {hasActiveSubscription ? t("billing.managePortal") : t("billing.upgrade")}
              </Button>
            )
          ) : null}
        </div>
      </CardContent>
    </Card>
  );
}

function BillingSkeleton() {
  return (
    <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-5" aria-hidden="true">
      {Array.from({ length: 5 }).map((_, i) => (
        <div key={i} className="flex flex-col gap-3 rounded-xl border border-border bg-card p-4">
          <Skeleton className="h-5 w-20" />
          <Skeleton className="h-8 w-16" />
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-full" />
          <Skeleton className="h-3 w-3/4" />
          <Skeleton className="mt-2 h-9 w-full" />
        </div>
      ))}
    </div>
  );
}
