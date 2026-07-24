import { AlertTriangle, Plus, RotateCw, Radio, Trash2 } from "lucide-react";
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
import { Skeleton } from "@/components/ui/skeleton";
import { PageHeader } from "@/components/PageHeader";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isUnauthorized } from "@/lib/mutation-error";
import { useCreatePixel, useDeletePixel, usePixels } from "@/lib/queries";
import type { Pixel, PixelProvider } from "@/lib/types";

interface FormErrors {
  measurementId?: string;
  apiSecret?: string;
  pixelId?: string;
  accessToken?: string;
  form?: string;
}

/** Friendly error message for the pixel remove mutation. */
function mutationErrorMessage(err: unknown, fallbackKey: MessageKey, t: (key: MessageKey) => string): string {
  if (err instanceof ApiError && err.status === 429) return t("common.rateLimited");
  return t(fallbackKey);
}

export function Pixels() {
  const t = useT();
  const [dialogOpen, setDialogOpen] = useState(false);
  const [removingId, setRemovingId] = useState<number | null>(null);
  const query = usePixels();
  const deletePixel = useDeletePixel();

  const pixels = query.data?.pixels ?? [];

  async function handleConfirmRemove() {
    if (removingId == null) return;
    try {
      await deletePixel.mutateAsync(removingId);
      toast.success(t("pixels.removeSuccess"));
      setRemovingId(null);
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(mutationErrorMessage(err, "pixels.removeGenericError", t));
    }
  }

  return (
    <div className="flex flex-col gap-4 animate-rise">
      <PageHeader
        title={t("pixels.heading")}
        subtitle={t("pixels.subtitle")}
        actions={
          <Button type="button" onClick={() => setDialogOpen(true)}>
            <Plus className="size-4" />
            {t("pixels.addButton")}
          </Button>
        }
      />

      {query.isPending && <PixelsSkeleton />}

      {query.isError && (
        <Card className="border-destructive/30">
          <CardContent className="flex flex-col items-center gap-3 py-8 text-center">
            <AlertTriangle className="size-8 text-destructive" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("pixels.loadError")}</p>
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

      {!query.isPending && !query.isError && pixels.length === 0 && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <div className="flex size-10 items-center justify-center rounded-[9px] bg-secondary">
              <Radio className="size-[18px] text-muted-foreground" aria-hidden="true" />
            </div>
            <div>
              <p className="font-medium">{t("pixels.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("pixels.emptySubtitle")}</p>
            </div>
            <Button type="button" onClick={() => setDialogOpen(true)}>
              <Plus className="size-4" />
              {t("pixels.addButton")}
            </Button>
          </CardContent>
        </Card>
      )}

      {!query.isPending && !query.isError && pixels.length > 0 && (
        <ul className="flex flex-col gap-2.5" aria-label={t("pixels.heading")}>
          {pixels.map((pixel) => (
            <PixelRow key={pixel.id} pixel={pixel} onRemove={() => setRemovingId(pixel.id)} />
          ))}
        </ul>
      )}

      <AddPixelDialog open={dialogOpen} onOpenChange={setDialogOpen} />

      <AlertDialog open={removingId != null} onOpenChange={(open) => !open && setRemovingId(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("pixels.removeTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("pixels.removeDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={deletePixel.isPending}>{t("common.cancel")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              disabled={deletePixel.isPending}
              onClick={handleConfirmRemove}
            >
              {deletePixel.isPending ? t("pixels.removing") : t("pixels.remove")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}

/**
 * A pixel has no user-chosen name, so the identifying credential (GA4's
 * Measurement ID or Meta's Pixel ID — the value that tells two pixels of the
 * same provider apart) takes the strong/name slot, mirroring how Domains
 * shows `host` there. The masked secret (API secret / access token) keeps
 * its own muted line below; it's still surfaced, just never in the clear.
 */
function PixelRow({ pixel, onRemove }: { pixel: Pixel; onRemove: () => void }) {
  const t = useT();
  const isGa4 = pixel.provider === "ga4";
  const idValue = isGa4 ? pixel.credentials.measurement_id : pixel.credentials.pixel_id;
  const secretLabel = isGa4 ? t("pixels.apiSecretField") : t("pixels.accessTokenField");
  const secretValue = isGa4 ? pixel.credentials.api_secret : pixel.credentials.access_token;

  return (
    <li data-testid="pixel-card" className="card-hover flex flex-col gap-3 rounded-lg border border-border bg-card p-4 shadow-card">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex min-w-0 items-center gap-2">
          <span className="min-w-0 truncate font-mono text-[14.5px] font-semibold text-strong">
            {idValue ?? "—"}
          </span>
          <Badge variant="secondary">
            {isGa4 ? t("pixels.providerBadgeGa4") : t("pixels.providerBadgeMeta")}
          </Badge>
          <Badge variant={pixel.active ? "default" : "outline"}>
            {pixel.active ? t("pixels.activeLabel") : t("pixels.inactiveLabel")}
          </Badge>
        </div>
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="shrink-0 text-destructive hover:text-destructive"
          aria-label={t("pixels.removeAria")}
          onClick={onRemove}
        >
          <Trash2 className="size-3.5" />
          {t("pixels.remove")}
        </Button>
      </div>
      <div className="font-mono text-[12.5px] text-muted-foreground">
        {secretLabel}: {secretValue ?? "—"}
      </div>
    </li>
  );
}

function PixelsSkeleton() {
  return (
    <div className="flex flex-col gap-2.5" aria-hidden="true">
      {Array.from({ length: 3 }).map((_, i) => (
        <Skeleton key={i} className="h-[72px] w-full" />
      ))}
    </div>
  );
}

interface AddPixelDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Create-pixel dialog: a provider select whose credential fields swap
 * between GA4 (Measurement ID + API secret) and Meta CAPI (Pixel ID +
 * Access token). Client-side required-field validation only — the server
 * is the source of truth for the actual per-provider requirement.
 */
function AddPixelDialog({ open, onOpenChange }: AddPixelDialogProps) {
  const t = useT();
  const [provider, setProvider] = useState<PixelProvider>("ga4");
  const [measurementId, setMeasurementId] = useState("");
  const [apiSecret, setApiSecret] = useState("");
  const [pixelId, setPixelId] = useState("");
  const [accessToken, setAccessToken] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const createPixel = useCreatePixel();

  function reset() {
    setProvider("ga4");
    setMeasurementId("");
    setApiSecret("");
    setPixelId("");
    setAccessToken("");
    setErrors({});
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (provider === "ga4") {
      if (!measurementId.trim()) next.measurementId = t("pixels.dialog.requiredField");
      if (!apiSecret.trim()) next.apiSecret = t("pixels.dialog.requiredField");
    } else {
      if (!pixelId.trim()) next.pixelId = t("pixels.dialog.requiredField");
      if (!accessToken.trim()) next.accessToken = t("pixels.dialog.requiredField");
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
      await createPixel.mutateAsync({
        provider,
        credentials:
          provider === "ga4"
            ? { measurement_id: measurementId.trim(), api_secret: apiSecret.trim() }
            : { pixel_id: pixelId.trim(), access_token: accessToken.trim() },
      });
      toast.success(t("pixels.dialog.successToast"));
      reset();
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("pixels.dialog.genericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("pixels.dialog.title")}</DialogTitle>
            <DialogDescription>{t("pixels.dialog.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="pixel-provider" className="text-sm font-medium">
                {t("pixels.dialog.providerLabel")}
              </label>
              <select
                id="pixel-provider"
                value={provider}
                onChange={(e) => setProvider(e.target.value as PixelProvider)}
                className="h-8 w-full rounded-lg border border-input bg-transparent px-2.5 py-1 text-base outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 md:text-sm dark:bg-input/30"
              >
                <option value="ga4">{t("pixels.dialog.providerGa4")}</option>
                <option value="meta_capi">{t("pixels.dialog.providerMeta")}</option>
              </select>
            </div>

            {provider === "ga4" ? (
              <>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="pixel-measurement-id" className="text-sm font-medium">
                    {t("pixels.dialog.measurementIdLabel")}
                  </label>
                  <Input
                    id="pixel-measurement-id"
                    type="text"
                    placeholder={t("pixels.dialog.measurementIdPlaceholder")}
                    value={measurementId}
                    onChange={(e) => setMeasurementId(e.target.value)}
                    aria-invalid={errors.measurementId != null}
                    autoFocus
                  />
                  {errors.measurementId && (
                    <p className="text-sm text-destructive" role="alert">
                      {errors.measurementId}
                    </p>
                  )}
                </div>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="pixel-api-secret" className="text-sm font-medium">
                    {t("pixels.dialog.apiSecretLabel")}
                  </label>
                  <Input
                    id="pixel-api-secret"
                    type="password"
                    placeholder={t("pixels.dialog.apiSecretPlaceholder")}
                    value={apiSecret}
                    onChange={(e) => setApiSecret(e.target.value)}
                    aria-invalid={errors.apiSecret != null}
                  />
                  {errors.apiSecret && (
                    <p className="text-sm text-destructive" role="alert">
                      {errors.apiSecret}
                    </p>
                  )}
                </div>
              </>
            ) : (
              <>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="pixel-id" className="text-sm font-medium">
                    {t("pixels.dialog.pixelIdLabel")}
                  </label>
                  <Input
                    id="pixel-id"
                    type="text"
                    placeholder={t("pixels.dialog.pixelIdPlaceholder")}
                    value={pixelId}
                    onChange={(e) => setPixelId(e.target.value)}
                    aria-invalid={errors.pixelId != null}
                    autoFocus
                  />
                  {errors.pixelId && (
                    <p className="text-sm text-destructive" role="alert">
                      {errors.pixelId}
                    </p>
                  )}
                </div>
                <div className="flex flex-col gap-1.5">
                  <label htmlFor="pixel-access-token" className="text-sm font-medium">
                    {t("pixels.dialog.accessTokenLabel")}
                  </label>
                  <Input
                    id="pixel-access-token"
                    type="password"
                    placeholder={t("pixels.dialog.accessTokenPlaceholder")}
                    value={accessToken}
                    onChange={(e) => setAccessToken(e.target.value)}
                    aria-invalid={errors.accessToken != null}
                  />
                  {errors.accessToken && (
                    <p className="text-sm text-destructive" role="alert">
                      {errors.accessToken}
                    </p>
                  )}
                </div>
              </>
            )}

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
  );
}
