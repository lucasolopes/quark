import { ChevronDown, ChevronRight, Plus, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isHttpUrl } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { usePatchLink, useLinkAlert, useSetLinkAlert, useDeleteLinkAlert } from "@/lib/queries";
import { formatTagsInput, parseTagsInput } from "@/lib/tags";
import { draftsFromRules, parseRuleDrafts, type RuleDraft } from "@/lib/rules";
import type { Folder, Link, Variant } from "@/lib/types";
import { RulesEditor } from "@/components/RulesEditor";

/** Same cap enforced server-side (`MAX_VARIANTS` in `src/api.rs`). */
const MAX_VARIANTS = 10;

interface VariantRow {
  url: string;
  weight: string;
}

function toVariantRows(variants: Variant[]): VariantRow[] {
  return variants.map((v) => ({ url: v.url, weight: String(v.weight) }));
}

function emptyVariantRow(): VariantRow {
  return { url: "", weight: "1" };
}

interface FormErrors {
  url?: string;
  ttl?: string;
  maxVisits?: string;
  rules?: string;
  appIos?: string;
  appAndroid?: string;
  fallbackUrl?: string;
  form?: string;
  variants?: string;
}

interface AlertFormErrors {
  threshold?: string;
  window?: string;
}

interface EditLinkDialogProps {
  link: Link;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Existing folders (from `useFolders`, lifted to the parent) offered in the folder field's datalist. */
  folders?: Folder[];
}

/**
 * Dialog for editing an existing link. Mounted with `key={link.code}` by the
 * caller (Links.tsx) so the fields always start from the right link — without
 * that we'd need to sync state via an effect on every link change.
 */
export function EditLinkDialog({ link, open, onOpenChange, folders = [] }: EditLinkDialogProps) {
  const t = useT();
  const [url, setUrl] = useState(link.url);
  const [ttl, setTtl] = useState("");
  const [removeExpiry, setRemoveExpiry] = useState(false);
  const [tagsInput, setTagsInput] = useState(formatTagsInput(link.tags ?? []));
  const [folder, setFolder] = useState(link.folder ?? "");
  const [maxVisits, setMaxVisits] = useState(link.max_visits ? String(link.max_visits) : "");
  const [ruleDrafts, setRuleDrafts] = useState<RuleDraft[]>(() => draftsFromRules(link.rules));
  const [showVariants, setShowVariants] = useState(link.variants.length > 0);
  const [variantRows, setVariantRows] = useState<VariantRow[]>(() => toVariantRows(link.variants));
  const [appIos, setAppIos] = useState(link.app_ios ?? "");
  const [appAndroid, setAppAndroid] = useState(link.app_android ?? "");
  const [fallbackUrl, setFallbackUrl] = useState(link.fallback_url ?? "");
  const [password, setPassword] = useState("");
  const [removePassword, setRemovePassword] = useState(false);
  const [errors, setErrors] = useState<FormErrors>({});
  const [schedulingOpen, setSchedulingOpen] = useState(false);
  const [appRedirectOpen, setAppRedirectOpen] = useState(false);
  const [passwordOpen, setPasswordOpen] = useState(false);
  const patchLink = usePatchLink();

  // Click-threshold alert (LUC-66): a collapsible section fetching the
  // current rule lazily (only once expanded), so opening the dialog never
  // fires an extra request for operators who don't touch it.
  const [showAlert, setShowAlert] = useState(false);
  const [alertThreshold, setAlertThreshold] = useState("");
  const [alertMinutes, setAlertMinutes] = useState("");
  const [alertPrefilled, setAlertPrefilled] = useState(false);
  const [alertErrors, setAlertErrors] = useState<AlertFormErrors>({});
  const alertQuery = useLinkAlert(link.code, { enabled: showAlert });
  const setLinkAlert = useSetLinkAlert();
  const deleteLinkAlert = useDeleteLinkAlert();

  useEffect(() => {
    if (alertQuery.data === undefined || alertPrefilled) return;
    if (alertQuery.data) {
      setAlertThreshold(String(alertQuery.data.threshold));
      setAlertMinutes(String(Math.max(1, Math.round(alertQuery.data.window_secs / 60))));
    }
    setAlertPrefilled(true);
  }, [alertQuery.data, alertPrefilled]);

  function addVariantRow() {
    setVariantRows((rows) => (rows.length >= MAX_VARIANTS ? rows : [...rows, emptyVariantRow()]));
  }

  function removeVariantRow(index: number) {
    setVariantRows((rows) => rows.filter((_, i) => i !== index));
  }

  function updateVariantRow(index: number, patch: Partial<VariantRow>) {
    setVariantRows((rows) => rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function formatExpiry(expiry: number | null): string {
    if (expiry == null) return t("dialogs.edit.neverExpires");
    return t("dialogs.edit.expiresOn", { date: new Date(expiry * 1000).toLocaleDateString("pt-BR") });
  }

  function formatCurrentMaxVisits(value?: number): string {
    return value ? String(value) : t("dialogs.edit.unlimitedVisits");
  }

  function handleOpenChange(next: boolean) {
    if (!next) setErrors({});
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = t("dialogs.edit.urlRequired");
    } else if (!isHttpUrl(url)) {
      next.url = t("dialogs.edit.urlInvalid");
    }
    const trimmedTtl = ttl.trim();
    if (!removeExpiry && trimmedTtl) {
      const n = Number(trimmedTtl);
      if (!Number.isInteger(n) || n <= 0) {
        next.ttl = t("dialogs.edit.ttlInvalid");
      }
    }
    const trimmedMaxVisits = maxVisits.trim();
    if (trimmedMaxVisits) {
      const n = Number(trimmedMaxVisits);
      if (!Number.isInteger(n) || n <= 0) {
        next.maxVisits = t("dialogs.edit.maxVisitsInvalid");
      }
    }
    if (variantRows.length > MAX_VARIANTS) {
      next.variants = t("dialogs.edit.tooManyVariants", { max: MAX_VARIANTS });
    } else {
      for (const row of variantRows) {
        if (!row.url.trim() || !isHttpUrl(row.url)) {
          next.variants = t("dialogs.edit.variantUrlInvalid");
          break;
        }
        const w = Number(row.weight.trim());
        if (!Number.isInteger(w) || w <= 0) {
          next.variants = t("dialogs.edit.variantWeightInvalid");
          break;
        }
      }
    }
    if (appIos.trim() && !isHttpUrl(appIos)) {
      next.appIos = t("dialogs.edit.appDestInvalid");
    }
    if (appAndroid.trim() && !isHttpUrl(appAndroid)) {
      next.appAndroid = t("dialogs.edit.appDestInvalid");
    }
    if (fallbackUrl.trim() && !isHttpUrl(fallbackUrl)) {
      next.fallbackUrl = t("dialogs.edit.fallbackUrlInvalid");
    }
    return next;
  }

  function buildVariants(): Variant[] {
    return variantRows.map((row) => ({ url: row.url.trim(), weight: Number(row.weight.trim()) }));
  }

  function validateAlert(): AlertFormErrors {
    const next: AlertFormErrors = {};
    const threshold = Number(alertThreshold.trim());
    if (!alertThreshold.trim() || !Number.isInteger(threshold) || threshold < 1) {
      next.threshold = t("dialogs.edit.alertThresholdInvalid");
    }
    const minutes = Number(alertMinutes.trim());
    if (!alertMinutes.trim() || !Number.isInteger(minutes) || minutes < 1) {
      next.window = t("dialogs.edit.alertWindowInvalid");
    }
    return next;
  }

  async function handleSaveAlert() {
    const nextErrors = validateAlert();
    if (Object.keys(nextErrors).length > 0) {
      setAlertErrors(nextErrors);
      return;
    }
    setAlertErrors({});
    try {
      await setLinkAlert.mutateAsync({
        code: link.code,
        body: { threshold: Number(alertThreshold.trim()), window_secs: Number(alertMinutes.trim()) * 60 },
      });
      toast.success(t("dialogs.edit.alertSaveSuccess"));
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(t("dialogs.edit.alertGenericError"));
    }
  }

  async function handleRemoveAlert() {
    try {
      await deleteLinkAlert.mutateAsync(link.code);
      setAlertThreshold("");
      setAlertMinutes("");
      toast.success(t("dialogs.edit.alertRemoveSuccess"));
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(t("dialogs.edit.alertGenericError"));
    }
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const nextErrors = validate();
    const { rules, error: rulesError } = parseRuleDrafts(ruleDrafts);
    if (rulesError) {
      nextErrors.rules = t(rulesError === "invalidUrl" ? "rules.rowInvalidUrl" : "rules.rowIncomplete");
    }
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }
    setErrors({});
    try {
      await patchLink.mutateAsync({
        code: link.code,
        body: {
          url: url.trim(),
          ...(removeExpiry ? { ttl: null } : ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
          tags: parseTagsInput(tagsInput),
          ...(maxVisits.trim()
            ? { max_visits: Number(maxVisits.trim()) }
            : link.max_visits
              ? { max_visits: null }
              : {}),
          rules,
          variants: buildVariants(),
          ...(appIos.trim() ? { app_ios: appIos.trim() } : link.app_ios?.trim() ? { app_ios: null } : {}),
          ...(appAndroid.trim() ? { app_android: appAndroid.trim() } : link.app_android?.trim() ? { app_android: null } : {}),
          ...(folder.trim() ? { folder: folder.trim() } : link.folder?.trim() ? { folder: null } : {}),
          ...(fallbackUrl.trim() ? { fallback_url: fallbackUrl.trim() } : link.fallback_url?.trim() ? { fallback_url: null } : {}),
          ...(removePassword ? { password: null } : password.trim() ? { password: password.trim() } : {}),
        },
      });
      toast.success(t("dialogs.edit.successToast"));
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 403) {
        setErrors({ url: t("dialogs.edit.forbiddenDestination") });
      } else if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("dialogs.edit.genericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        <form onSubmit={handleSubmit} className="flex max-h-[85vh] flex-col">
          <DialogHeader className="shrink-0">
            <DialogTitle>{t("dialogs.edit.title", { code: link.code })}</DialogTitle>
            <DialogDescription>{t("dialogs.edit.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-url" className="text-sm font-medium">
                {t("dialogs.edit.urlLabel")}
              </label>
              <Input
                id="edit-link-url"
                type="text"
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

            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-tags" className="text-sm font-medium">
                {t("dialogs.edit.tagsLabel")} <span className="text-muted-foreground">({t("dialogs.edit.tagsHint")})</span>
              </label>
              <Input
                id="edit-link-tags"
                type="text"
                placeholder={t("dialogs.edit.tagsPlaceholder")}
                value={tagsInput}
                onChange={(e) => setTagsInput(e.target.value)}
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="edit-link-folder" className="text-sm font-medium">
                {t("dialogs.edit.folderLabel")} <span className="text-muted-foreground">{t("dialogs.edit.folderOptional")}</span>
              </label>
              <Input
                id="edit-link-folder"
                type="text"
                list="edit-link-folder-options"
                placeholder={t("dialogs.edit.folderPlaceholder")}
                value={folder}
                onChange={(e) => setFolder(e.target.value)}
              />
              <datalist id="edit-link-folder-options">
                {folders.map((f) => (
                  <option key={f.name} value={f.name} />
                ))}
              </datalist>
            </div>

            <div className="flex flex-col gap-2 rounded-lg border border-input p-2.5">
              <button
                type="button"
                className="flex items-center gap-1.5 text-sm font-medium"
                aria-expanded={schedulingOpen}
                onClick={() => setSchedulingOpen((open) => !open)}
              >
                {schedulingOpen ? (
                  <ChevronDown className="size-4 text-muted-foreground" aria-hidden />
                ) : (
                  <ChevronRight className="size-4 text-muted-foreground" aria-hidden />
                )}
                {t("dialogs.sections.scheduling")}
              </button>

              {schedulingOpen && (
                <div className="flex flex-col gap-3 pt-1">
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="edit-link-ttl" className="text-sm font-medium">
                      {t("dialogs.edit.ttlLabel")} <span className="text-muted-foreground">{t("dialogs.edit.ttlOptional")}</span>
                    </label>
                    <Input
                      id="edit-link-ttl"
                      type="number"
                      min={1}
                      step={1}
                      placeholder={t("dialogs.edit.ttlPlaceholder", { expiry: formatExpiry(link.expiry) })}
                      value={ttl}
                      onChange={(e) => setTtl(e.target.value)}
                      aria-invalid={errors.ttl != null}
                      disabled={removeExpiry}
                    />
                    {errors.ttl && (
                      <p className="text-sm text-destructive" role="alert">
                        {errors.ttl}
                      </p>
                    )}
                    <label className="flex items-center gap-2 text-sm text-muted-foreground">
                      <input
                        type="checkbox"
                        className="size-4 rounded border-input accent-primary"
                        checked={removeExpiry}
                        onChange={(e) => {
                          setRemoveExpiry(e.target.checked);
                          if (e.target.checked) setTtl("");
                        }}
                      />
                      {t("dialogs.edit.removeExpiryLabel")}
                    </label>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="edit-link-max-visits" className="text-sm font-medium">
                      {t("dialogs.edit.maxVisitsLabel")} <span className="text-muted-foreground">{t("dialogs.edit.maxVisitsOptional")}</span>
                    </label>
                    <Input
                      id="edit-link-max-visits"
                      type="number"
                      min={1}
                      step={1}
                      placeholder={t("dialogs.edit.maxVisitsPlaceholder", { current: formatCurrentMaxVisits(link.max_visits) })}
                      value={maxVisits}
                      onChange={(e) => setMaxVisits(e.target.value)}
                      aria-invalid={errors.maxVisits != null}
                    />
                    {errors.maxVisits && (
                      <p className="text-sm text-destructive" role="alert">
                        {errors.maxVisits}
                      </p>
                    )}
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="edit-link-fallback-url" className="text-sm font-medium">
                      {t("dialogs.edit.fallbackUrlLabel")} <span className="text-muted-foreground">{t("dialogs.edit.fallbackUrlOptional")}</span>
                    </label>
                    <p className="text-sm text-muted-foreground">{t("dialogs.edit.fallbackUrlNote")}</p>
                    <Input
                      id="edit-link-fallback-url"
                      type="text"
                      placeholder={t("dialogs.edit.fallbackUrlPlaceholder")}
                      value={fallbackUrl}
                      onChange={(e) => setFallbackUrl(e.target.value)}
                      aria-invalid={errors.fallbackUrl != null}
                    />
                    {errors.fallbackUrl && (
                      <p className="text-sm text-destructive" role="alert">
                        {errors.fallbackUrl}
                      </p>
                    )}
                  </div>
                </div>
              )}
            </div>

            <div className="flex flex-col gap-2 rounded-lg border border-input p-2.5">
              <button
                type="button"
                className="flex items-center gap-1.5 text-sm font-medium"
                aria-expanded={appRedirectOpen}
                onClick={() => setAppRedirectOpen((open) => !open)}
              >
                {appRedirectOpen ? (
                  <ChevronDown className="size-4 text-muted-foreground" aria-hidden />
                ) : (
                  <ChevronRight className="size-4 text-muted-foreground" aria-hidden />
                )}
                {t("dialogs.sections.appRedirect")}
              </button>

              {appRedirectOpen && (
                <div className="flex flex-col gap-3 pt-1">
                  <p className="text-sm text-muted-foreground">{t("dialogs.edit.appDestNote")}</p>
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="edit-link-app-ios" className="text-sm font-medium">
                      {t("dialogs.edit.appIosLabel")}
                    </label>
                    <Input
                      id="edit-link-app-ios"
                      type="text"
                      placeholder={t("dialogs.edit.appIosPlaceholder")}
                      value={appIos}
                      onChange={(e) => setAppIos(e.target.value)}
                      aria-invalid={errors.appIos != null}
                    />
                    {errors.appIos && (
                      <p className="text-sm text-destructive" role="alert">
                        {errors.appIos}
                      </p>
                    )}
                  </div>
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="edit-link-app-android" className="text-sm font-medium">
                      {t("dialogs.edit.appAndroidLabel")}
                    </label>
                    <Input
                      id="edit-link-app-android"
                      type="text"
                      placeholder={t("dialogs.edit.appAndroidPlaceholder")}
                      value={appAndroid}
                      onChange={(e) => setAppAndroid(e.target.value)}
                      aria-invalid={errors.appAndroid != null}
                    />
                    {errors.appAndroid && (
                      <p className="text-sm text-destructive" role="alert">
                        {errors.appAndroid}
                      </p>
                    )}
                  </div>
                </div>
              )}
            </div>

            <div className="flex flex-col gap-2 rounded-lg border border-input p-2.5">
              <button
                type="button"
                className="flex items-center gap-1.5 text-sm font-medium"
                aria-expanded={passwordOpen}
                onClick={() => setPasswordOpen((open) => !open)}
              >
                {passwordOpen ? (
                  <ChevronDown className="size-4 text-muted-foreground" aria-hidden />
                ) : (
                  <ChevronRight className="size-4 text-muted-foreground" aria-hidden />
                )}
                {t("dialogs.sections.password")}
              </button>

              {passwordOpen && (
                <div className="flex flex-col gap-3 pt-1">
                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="edit-link-password" className="text-sm font-medium">
                      {t("dialogs.edit.passwordLabel")}
                    </label>
                    <p className="text-sm text-muted-foreground">
                      {link.has_password ? t("dialogs.edit.passwordNoteProtected") : t("dialogs.edit.passwordNote")}
                    </p>
                    <Input
                      id="edit-link-password"
                      type="password"
                      autoComplete="new-password"
                      placeholder={
                        link.has_password
                          ? t("dialogs.edit.passwordPlaceholderProtected")
                          : t("dialogs.edit.passwordPlaceholder")
                      }
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                      disabled={removePassword}
                    />
                    {link.has_password && (
                      <label className="flex items-center gap-2 text-sm text-muted-foreground">
                        <input
                          type="checkbox"
                          className="size-4 rounded border-input accent-primary"
                          checked={removePassword}
                          onChange={(e) => {
                            setRemovePassword(e.target.checked);
                            if (e.target.checked) setPassword("");
                          }}
                        />
                        {t("dialogs.edit.removePasswordLabel")}
                      </label>
                    )}
                  </div>
                </div>
              )}
            </div>

            <div className="flex flex-col gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="self-start"
                aria-expanded={showVariants}
                onClick={() => setShowVariants((v) => !v)}
              >
                {t("dialogs.edit.variantsToggle")}
              </Button>

              {showVariants && (
                <div className="flex flex-col gap-2 rounded-md border border-border p-3">
                  <p className="text-sm text-muted-foreground">{t("dialogs.edit.variantsHint")}</p>

                  {variantRows.map((row, i) => (
                    <div key={i} className="flex items-end gap-2">
                      <div className="flex flex-1 flex-col gap-1.5">
                        <label htmlFor={`edit-variant-url-${i}`} className="sr-only">
                          {t("dialogs.edit.variantUrlLabel")}
                        </label>
                        <Input
                          id={`edit-variant-url-${i}`}
                          type="text"
                          placeholder={t("dialogs.edit.variantUrlPlaceholder")}
                          value={row.url}
                          onChange={(e) => updateVariantRow(i, { url: e.target.value })}
                        />
                      </div>
                      <div className="flex w-20 flex-col gap-1.5">
                        <label htmlFor={`edit-variant-weight-${i}`} className="sr-only">
                          {t("dialogs.edit.variantWeightLabel")}
                        </label>
                        <Input
                          id={`edit-variant-weight-${i}`}
                          type="number"
                          min={1}
                          step={1}
                          placeholder={t("dialogs.edit.variantWeightLabel")}
                          value={row.weight}
                          onChange={(e) => updateVariantRow(i, { weight: e.target.value })}
                        />
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("dialogs.edit.removeVariant")}
                        onClick={() => removeVariantRow(i)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  ))}

                  {errors.variants && (
                    <p className="text-sm text-destructive" role="alert">
                      {errors.variants}
                    </p>
                  )}

                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="self-start"
                    disabled={variantRows.length >= MAX_VARIANTS}
                    onClick={addVariantRow}
                  >
                    <Plus className="size-3.5" />
                    {t("dialogs.edit.addVariant")}
                  </Button>
                </div>
              )}
            </div>

            <RulesEditor idPrefix="edit-link" drafts={ruleDrafts} onChange={setRuleDrafts} />
            {errors.rules && (
              <p className="text-sm text-destructive" role="alert">
                {errors.rules}
              </p>
            )}

            <div className="flex flex-col gap-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="self-start"
                aria-expanded={showAlert}
                onClick={() => setShowAlert((v) => !v)}
              >
                {t("dialogs.edit.alertToggle")}
              </Button>

              {showAlert && (
                <div className="flex flex-col gap-2 rounded-md border border-border p-3">
                  <p className="text-sm text-muted-foreground">{t("dialogs.edit.alertNote")}</p>

                  {alertQuery.isLoading ? (
                    <p className="text-sm text-muted-foreground">{t("dialogs.edit.alertLoading")}</p>
                  ) : alertQuery.data ? (
                    <p className="text-sm text-muted-foreground">
                      {t("dialogs.edit.alertCurrent", {
                        threshold: alertQuery.data.threshold,
                        minutes: Math.max(1, Math.round(alertQuery.data.window_secs / 60)),
                      })}
                    </p>
                  ) : (
                    <p className="text-sm text-muted-foreground">{t("dialogs.edit.alertNone")}</p>
                  )}

                  <div className="flex items-end gap-2">
                    <div className="flex flex-1 flex-col gap-1.5">
                      <label htmlFor="edit-link-alert-threshold" className="text-sm font-medium">
                        {t("dialogs.edit.alertThresholdLabel")}
                      </label>
                      <Input
                        id="edit-link-alert-threshold"
                        type="number"
                        min={1}
                        step={1}
                        value={alertThreshold}
                        onChange={(e) => setAlertThreshold(e.target.value)}
                        aria-invalid={alertErrors.threshold != null}
                      />
                    </div>
                    <div className="flex flex-1 flex-col gap-1.5">
                      <label htmlFor="edit-link-alert-window" className="text-sm font-medium">
                        {t("dialogs.edit.alertWindowLabel")}
                      </label>
                      <Input
                        id="edit-link-alert-window"
                        type="number"
                        min={1}
                        step={1}
                        value={alertMinutes}
                        onChange={(e) => setAlertMinutes(e.target.value)}
                        aria-invalid={alertErrors.window != null}
                      />
                    </div>
                  </div>
                  {alertErrors.threshold && (
                    <p className="text-sm text-destructive" role="alert">
                      {alertErrors.threshold}
                    </p>
                  )}
                  {alertErrors.window && (
                    <p className="text-sm text-destructive" role="alert">
                      {alertErrors.window}
                    </p>
                  )}

                  <div className="flex gap-2">
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={setLinkAlert.isPending}
                      onClick={handleSaveAlert}
                    >
                      {setLinkAlert.isPending ? t("dialogs.edit.alertSaving") : t("dialogs.edit.alertSave")}
                    </Button>
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      disabled={deleteLinkAlert.isPending}
                      onClick={handleRemoveAlert}
                    >
                      {deleteLinkAlert.isPending ? t("dialogs.edit.alertRemoving") : t("dialogs.edit.alertRemove")}
                    </Button>
                  </div>
                </div>
              )}
            </div>

            {errors.form && (
              <p className="text-sm text-destructive" role="alert">
                {errors.form}
              </p>
            )}
          </div>

          <DialogFooter className="shrink-0 pt-1">
            <Button type="button" variant="outline" onClick={() => handleOpenChange(false)}>
              {t("common.cancel")}
            </Button>
            <Button type="submit" disabled={patchLink.isPending}>
              {patchLink.isPending ? t("dialogs.edit.submitting") : t("dialogs.edit.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
