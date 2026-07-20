import { ChevronDown, ChevronRight, Plus, Trash2 } from "lucide-react";
import { useState } from "react";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isHttpUrl, isNumericCode } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { useCreateLink } from "@/lib/queries";
import { Combobox } from "@/components/Combobox";
import { applyUtm, deleteUtmTemplate, loadUtmTemplates, saveUtmTemplate, type UtmParams } from "@/lib/utm";
import { parseRuleDrafts, type RuleDraft } from "@/lib/rules";
import { RulesEditor } from "@/components/RulesEditor";
import { distributeEvenly, variantsPercentTotal, MAX_VARIANTS } from "@/lib/variants";
import { DurationField } from "@/components/DurationField";
import { DEFAULT_DURATION_UNIT, durationToSeconds } from "@/lib/duration";
import type { Folder, Variant } from "@/lib/types";

interface VariantRow {
  url: string;
  weight: string;
}

/** Reassign percentages so the rows split 100% evenly, keeping their URLs. */
function rebalance(rows: VariantRow[]): VariantRow[] {
  const pct = distributeEvenly(rows.length);
  return rows.map((row, i) => ({ ...row, weight: String(pct[i]) }));
}

interface FormErrors {
  url?: string;
  alias?: string;
  ttl?: string;
  maxVisits?: string;
  rules?: string;
  appIos?: string;
  appAndroid?: string;
  fallbackUrl?: string;
  form?: string;
  variants?: string;
}

const EMPTY_UTM: UtmParams = {};

/** Whether at least one UTM field carries a non-empty value. */
function hasAnyUtm(params: UtmParams): boolean {
  return Object.values(params).some((value) => value != null && value.trim() !== "");
}

interface CreateLinkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Existing folders (from `useFolders`, lifted to the parent) offered in the folder picker. */
  folders?: Folder[];
  /** Existing tag names (from `useTags`, lifted to the parent) offered in the tags picker. */
  tags?: string[];
}

/**
 * Short link creation dialog. Validates client-side (http/https URL, alias
 * outside the numeric-code space, positive TTL) before calling the API —
 * avoids a round-trip just to get back an error we already knew about.
 */
export function CreateLinkDialog({ open, onOpenChange, folders = [], tags: tagOptions = [] }: CreateLinkDialogProps) {
  const t = useT();
  const [url, setUrl] = useState("");
  const [alias, setAlias] = useState("");
  const [ttl, setTtl] = useState("");
  const [ttlUnit, setTtlUnit] = useState<string>(DEFAULT_DURATION_UNIT);
  const [tags, setTags] = useState<string[]>([]);
  const [folder, setFolder] = useState("");
  const [maxVisits, setMaxVisits] = useState("");
  const [ruleDrafts, setRuleDrafts] = useState<RuleDraft[]>([]);
  const [showVariants, setShowVariants] = useState(false);
  const [variantRows, setVariantRows] = useState<VariantRow[]>([]);
  const [appIos, setAppIos] = useState("");
  const [appAndroid, setAppAndroid] = useState("");
  const [fallbackUrl, setFallbackUrl] = useState("");
  const [password, setPassword] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const [schedulingOpen, setSchedulingOpen] = useState(false);
  const [appRedirectOpen, setAppRedirectOpen] = useState(false);
  const [passwordOpen, setPasswordOpen] = useState(false);
  const [utmOpen, setUtmOpen] = useState(false);
  const [utm, setUtm] = useState<UtmParams>(EMPTY_UTM);
  const [templates, setTemplates] = useState(() => loadUtmTemplates());
  const [templateName, setTemplateName] = useState("");
  const [templateNameError, setTemplateNameError] = useState<string | undefined>(undefined);
  const createLink = useCreateLink();

  function reset() {
    setUrl("");
    setAlias("");
    setTtl("");
    setTtlUnit(DEFAULT_DURATION_UNIT);
    setTags([]);
    setFolder("");
    setMaxVisits("");
    setRuleDrafts([]);
    setShowVariants(false);
    setVariantRows([]);
    setAppIos("");
    setAppAndroid("");
    setFallbackUrl("");
    setPassword("");
    setErrors({});
    setSchedulingOpen(false);
    setAppRedirectOpen(false);
    setPasswordOpen(false);
    setUtmOpen(false);
    setUtm(EMPTY_UTM);
    setTemplateName("");
    setTemplateNameError(undefined);
  }

  function setUtmField(field: keyof UtmParams, value: string) {
    setUtm((prev) => ({ ...prev, [field]: value }));
  }

  function applyTemplate(name: string) {
    const params = templates[name];
    if (params) setUtm(params);
  }

  function handleDeleteTemplate(name: string) {
    deleteUtmTemplate(name);
    setTemplates(loadUtmTemplates());
    toast.success(t("utm.templateDeleted"));
  }

  function handleSaveTemplate() {
    const trimmed = templateName.trim();
    if (!trimmed) {
      setTemplateNameError(t("utm.templateNameRequired"));
      return;
    }
    saveUtmTemplate(trimmed, utm);
    setTemplates(loadUtmTemplates());
    setTemplateName("");
    setTemplateNameError(undefined);
    toast.success(t("utm.templateSaved"));
  }

  const utmPreview = url.trim() ? applyUtm(url.trim(), utm) : "";
  const templateNames = Object.keys(templates);
  const variantsTotal = variantsPercentTotal(variantRows.map((r) => r.weight));
  const variantsTotalValid = variantRows.length === 0 || variantsTotal === 100;

  function addVariantRow() {
    setVariantRows((rows) => (rows.length >= MAX_VARIANTS ? rows : rebalance([...rows, { url: "", weight: "0" }])));
  }

  function removeVariantRow(index: number) {
    setVariantRows((rows) => rebalance(rows.filter((_, i) => i !== index)));
  }

  function updateVariantRow(index: number, patch: Partial<VariantRow>) {
    setVariantRows((rows) => rows.map((row, i) => (i === index ? { ...row, ...patch } : row)));
  }

  function distributeVariantsEvenly() {
    setVariantRows((rows) => rebalance(rows));
  }

  function handleOpenChange(next: boolean) {
    if (!next) reset();
    onOpenChange(next);
  }

  function validate(): FormErrors {
    const next: FormErrors = {};
    if (!url.trim()) {
      next.url = t("dialogs.create.urlRequired");
    } else if (!isHttpUrl(url)) {
      next.url = t("dialogs.create.urlInvalid");
    }
    const trimmedAlias = alias.trim();
    if (trimmedAlias && isNumericCode(trimmedAlias)) {
      next.alias = t("dialogs.create.aliasCollision");
    }
    if (ttl.trim() && durationToSeconds(ttl, ttlUnit) == null) {
      next.ttl = t("dialogs.create.ttlInvalid");
    }
    const trimmedMaxVisits = maxVisits.trim();
    if (trimmedMaxVisits) {
      const n = Number(trimmedMaxVisits);
      if (!Number.isInteger(n) || n <= 0) {
        next.maxVisits = t("dialogs.create.maxVisitsInvalid");
      }
    }
    if (variantRows.length > MAX_VARIANTS) {
      next.variants = t("dialogs.create.tooManyVariants", { max: MAX_VARIANTS });
    } else {
      let sum = 0;
      for (const row of variantRows) {
        if (!row.url.trim() || !isHttpUrl(row.url)) {
          next.variants = t("dialogs.create.variantUrlInvalid");
          break;
        }
        const w = Number(row.weight.trim());
        if (!Number.isInteger(w) || w <= 0) {
          next.variants = t("dialogs.create.variantPercentInvalid");
          break;
        }
        sum += w;
      }
      if (!next.variants && variantRows.length > 0 && sum !== 100) {
        next.variants = t("dialogs.create.variantsSumInvalid", { total: sum });
      }
    }
    if (appIos.trim() && !isHttpUrl(appIos)) {
      next.appIos = t("dialogs.create.appDestInvalid");
    }
    if (appAndroid.trim() && !isHttpUrl(appAndroid)) {
      next.appAndroid = t("dialogs.create.appDestInvalid");
    }
    if (fallbackUrl.trim() && !isHttpUrl(fallbackUrl)) {
      next.fallbackUrl = t("dialogs.create.fallbackUrlInvalid");
    }
    return next;
  }

  function buildVariants(): Variant[] {
    return variantRows.map((row) => ({ url: row.url.trim(), weight: Number(row.weight.trim()) }));
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
    const trimmedUrl = url.trim();
    const destination = hasAnyUtm(utm) ? applyUtm(trimmedUrl, utm) : trimmedUrl;
    try {
      const variants = buildVariants();
      const ttlSecs = durationToSeconds(ttl, ttlUnit);
      await createLink.mutateAsync({
        url: destination,
        ...(alias.trim() ? { alias: alias.trim() } : {}),
        ...(ttlSecs != null ? { ttl: ttlSecs } : {}),
        ...(tags.length > 0 ? { tags } : {}),
        ...(maxVisits.trim() ? { max_visits: Number(maxVisits.trim()) } : {}),
        ...(rules.length > 0 ? { rules } : {}),
        ...(variants.length > 0 ? { variants } : {}),
        ...(appIos.trim() ? { app_ios: appIos.trim() } : {}),
        ...(appAndroid.trim() ? { app_android: appAndroid.trim() } : {}),
        ...(folder.trim() ? { folder: folder.trim() } : {}),
        ...(fallbackUrl.trim() ? { fallback_url: fallbackUrl.trim() } : {}),
        ...(password.trim() ? { password: password.trim() } : {}),
      });
      toast.success(t("dialogs.create.successToast"));
      reset();
      onOpenChange(false);
    } catch (err) {
      if (isUnauthorized(err)) return;
      if (err instanceof ApiError && err.status === 409) {
        setErrors({ alias: t("dialogs.create.aliasInUse") });
      } else if (err instanceof ApiError && err.status === 403) {
        setErrors({ url: t("dialogs.create.forbiddenDestination") });
      } else if (err instanceof ApiError && err.status === 429) {
        toast.error(t("common.rateLimited"));
      } else {
        setErrors({ form: t("dialogs.create.genericError") });
      }
    }
  }

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-3xl">
        <form onSubmit={handleSubmit} className="flex max-h-[85vh] flex-col">
          <DialogHeader className="shrink-0">
            <DialogTitle>{t("dialogs.create.title")}</DialogTitle>
            <DialogDescription>{t("dialogs.create.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex min-h-0 flex-1 flex-col gap-3 overflow-y-auto py-3">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="create-link-url" className="text-sm font-medium">
                {t("dialogs.create.urlLabel")}
              </label>
              <Input
                id="create-link-url"
                type="text"
                placeholder={t("dialogs.create.urlPlaceholder")}
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

            <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
              <div className="flex flex-col gap-1.5">
                <label htmlFor="create-link-alias" className="text-sm font-medium">
                  {t("dialogs.create.aliasLabel")} <span className="text-muted-foreground">{t("dialogs.create.optional")}</span>
                </label>
                <Input
                  id="create-link-alias"
                  type="text"
                  placeholder={t("dialogs.create.aliasPlaceholder")}
                  value={alias}
                  onChange={(e) => setAlias(e.target.value)}
                  aria-invalid={errors.alias != null}
                />
                {errors.alias && (
                  <p className="text-sm text-destructive" role="alert">
                    {errors.alias}
                  </p>
                )}
              </div>

              <div className="flex flex-col gap-1.5">
                <label htmlFor="create-link-folder" className="text-sm font-medium">
                  {t("dialogs.create.folderLabel")} <span className="text-muted-foreground">{t("dialogs.create.optional")}</span>
                </label>
                <Combobox
                  id="create-link-folder"
                  createLabel={t("dialogs.create.folderCreate")}
                  options={folders.map((f) => ({ value: f.name, label: f.name }))}
                  value={folder ? [folder] : []}
                  onChange={(vals) => setFolder(vals[0] ?? "")}
                  ariaLabel={t("dialogs.create.folderLabel")}
                  placeholder={t("dialogs.create.folderPlaceholder")}
                />
              </div>
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="create-link-tags" className="text-sm font-medium">
                {t("dialogs.create.tagsLabel")} <span className="text-muted-foreground">({t("dialogs.create.tagsHint")})</span>
              </label>
              <Combobox
                id="create-link-tags"
                multiple
                createLabel={t("dialogs.create.tagsCreate")}
                options={tagOptions.map((name) => ({ value: name, label: name }))}
                value={tags}
                onChange={setTags}
                ariaLabel={t("dialogs.create.tagsLabel")}
                placeholder={t("dialogs.create.tagsPlaceholder")}
              />
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
                  <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                    <DurationField
                      id="create-link-ttl"
                      label={t("dialogs.create.ttlLabel")}
                      hint={t("dialogs.create.ttlOptional")}
                      value={ttl}
                      unit={ttlUnit}
                      onValueChange={setTtl}
                      onUnitChange={setTtlUnit}
                      placeholder={t("dialogs.create.ttlPlaceholder")}
                      error={errors.ttl}
                    />

                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="create-link-max-visits" className="text-sm font-medium">
                        {t("dialogs.create.maxVisitsLabel")} <span className="text-muted-foreground">{t("dialogs.create.maxVisitsOptional")}</span>
                      </label>
                      <Input
                        id="create-link-max-visits"
                        type="number"
                        min={1}
                        step={1}
                        placeholder={t("dialogs.create.maxVisitsPlaceholder")}
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
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="create-link-fallback-url" className="text-sm font-medium">
                      {t("dialogs.create.fallbackUrlLabel")} <span className="text-muted-foreground">{t("dialogs.create.optional")}</span>
                    </label>
                    <p className="text-sm text-muted-foreground">{t("dialogs.create.fallbackUrlNote")}</p>
                    <Input
                      id="create-link-fallback-url"
                      type="text"
                      placeholder={t("dialogs.create.fallbackUrlPlaceholder")}
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
                  <p className="text-sm text-muted-foreground">{t("dialogs.create.appDestNote")}</p>
                  <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="create-link-app-ios" className="text-sm font-medium">
                        {t("dialogs.create.appIosLabel")}
                      </label>
                      <Input
                        id="create-link-app-ios"
                        type="text"
                        placeholder={t("dialogs.create.appIosPlaceholder")}
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
                      <label htmlFor="create-link-app-android" className="text-sm font-medium">
                        {t("dialogs.create.appAndroidLabel")}
                      </label>
                      <Input
                        id="create-link-app-android"
                        type="text"
                        placeholder={t("dialogs.create.appAndroidPlaceholder")}
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
                    <label htmlFor="create-link-password" className="text-sm font-medium">
                      {t("dialogs.create.passwordLabel")} <span className="text-muted-foreground">{t("dialogs.create.optional")}</span>
                    </label>
                    <p className="text-sm text-muted-foreground">{t("dialogs.create.passwordNote")}</p>
                    <Input
                      id="create-link-password"
                      type="password"
                      autoComplete="new-password"
                      placeholder={t("dialogs.create.passwordPlaceholder")}
                      value={password}
                      onChange={(e) => setPassword(e.target.value)}
                    />
                  </div>
                </div>
              )}
            </div>

            <div className="flex flex-col gap-2 rounded-lg border border-input p-2.5">
              <button
                type="button"
                className="flex items-center gap-1.5 text-sm font-medium"
                aria-expanded={utmOpen}
                onClick={() => setUtmOpen((open) => !open)}
              >
                {utmOpen ? (
                  <ChevronDown className="size-4 text-muted-foreground" aria-hidden />
                ) : (
                  <ChevronRight className="size-4 text-muted-foreground" aria-hidden />
                )}
                {t("utm.sectionTitle")}
              </button>

              {utmOpen && (
                <div className="flex flex-col gap-3 pt-1">
                  <p className="text-xs text-muted-foreground">{t("utm.sectionSubtitle")}</p>

                  <div className="flex items-center gap-2">
                    <DropdownMenu>
                      <DropdownMenuTrigger
                        render={
                          <Button type="button" variant="outline" size="sm">
                            {t("utm.templatesLabel")}
                          </Button>
                        }
                      />
                      <DropdownMenuContent align="start">
                        {templateNames.length === 0 && (
                          <div className="px-1.5 py-1 text-xs text-muted-foreground">
                            {t("utm.templatesEmpty")}
                          </div>
                        )}
                        {templateNames.map((name) => (
                          <DropdownMenuItem
                            key={name}
                            onClick={() => applyTemplate(name)}
                            className="flex items-center justify-between gap-2"
                          >
                            <span>{name}</span>
                            <button
                              type="button"
                              aria-label={t("utm.deleteTemplateAria", { name })}
                              className="text-muted-foreground hover:text-destructive"
                              onClick={(e) => {
                                e.stopPropagation();
                                handleDeleteTemplate(name);
                              }}
                            >
                              <Trash2 className="size-3.5" aria-hidden />
                            </button>
                          </DropdownMenuItem>
                        ))}
                      </DropdownMenuContent>
                    </DropdownMenu>
                  </div>

                  <div className="grid grid-cols-1 gap-3 sm:grid-cols-2">
                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="utm-source" className="text-sm font-medium">
                        {t("utm.sourceLabel")}
                      </label>
                      <Input
                        id="utm-source"
                        type="text"
                        placeholder={t("utm.sourcePlaceholder")}
                        value={utm.source ?? ""}
                        onChange={(e) => setUtmField("source", e.target.value)}
                      />
                    </div>

                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="utm-medium" className="text-sm font-medium">
                        {t("utm.mediumLabel")}
                      </label>
                      <Input
                        id="utm-medium"
                        type="text"
                        placeholder={t("utm.mediumPlaceholder")}
                        value={utm.medium ?? ""}
                        onChange={(e) => setUtmField("medium", e.target.value)}
                      />
                    </div>

                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="utm-campaign" className="text-sm font-medium">
                        {t("utm.campaignLabel")}
                      </label>
                      <Input
                        id="utm-campaign"
                        type="text"
                        placeholder={t("utm.campaignPlaceholder")}
                        value={utm.campaign ?? ""}
                        onChange={(e) => setUtmField("campaign", e.target.value)}
                      />
                    </div>

                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="utm-term" className="text-sm font-medium">
                        {t("utm.termLabel")}
                      </label>
                      <Input
                        id="utm-term"
                        type="text"
                        placeholder={t("utm.termPlaceholder")}
                        value={utm.term ?? ""}
                        onChange={(e) => setUtmField("term", e.target.value)}
                      />
                    </div>

                    <div className="flex flex-col gap-1.5">
                      <label htmlFor="utm-content" className="text-sm font-medium">
                        {t("utm.contentLabel")}
                      </label>
                      <Input
                        id="utm-content"
                        type="text"
                        placeholder={t("utm.contentPlaceholder")}
                        value={utm.content ?? ""}
                        onChange={(e) => setUtmField("content", e.target.value)}
                      />
                    </div>
                  </div>

                  <div className="flex flex-col gap-1.5">
                    <label htmlFor="utm-template-name" className="text-sm font-medium">
                      {t("utm.templateNameLabel")}
                    </label>
                    <div className="flex items-center gap-2">
                      <Input
                        id="utm-template-name"
                        type="text"
                        placeholder={t("utm.templateNamePlaceholder")}
                        value={templateName}
                        onChange={(e) => {
                          setTemplateName(e.target.value);
                          if (templateNameError) setTemplateNameError(undefined);
                        }}
                      />
                      <Button type="button" variant="outline" size="sm" onClick={handleSaveTemplate}>
                        {t("utm.saveAsTemplate")}
                      </Button>
                    </div>
                    {templateNameError && (
                      <p className="text-sm text-destructive" role="alert">
                        {templateNameError}
                      </p>
                    )}
                  </div>

                  {utmPreview && (
                    <div className="flex flex-col gap-1">
                      <span className="text-xs font-medium text-muted-foreground">
                        {t("utm.previewLabel")}
                      </span>
                      <p className="break-all text-sm">{utmPreview}</p>
                    </div>
                  )}
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
                {t("dialogs.create.variantsToggle")}
              </Button>

              {showVariants && (
                <div className="flex flex-col gap-2 rounded-md border border-border p-3">
                  <p className="text-sm text-muted-foreground">{t("dialogs.create.variantsHint")}</p>

                  {variantRows.map((row, i) => (
                    <div key={i} className="flex items-end gap-2">
                      <div className="flex flex-1 flex-col gap-1.5">
                        <label htmlFor={`create-variant-url-${i}`} className="sr-only">
                          {t("dialogs.create.variantUrlLabel")}
                        </label>
                        <Input
                          id={`create-variant-url-${i}`}
                          type="text"
                          placeholder={t("dialogs.create.variantUrlPlaceholder")}
                          value={row.url}
                          onChange={(e) => updateVariantRow(i, { url: e.target.value })}
                        />
                      </div>
                      <div className="flex w-24 flex-col gap-1.5">
                        <label htmlFor={`create-variant-weight-${i}`} className="sr-only">
                          {t("dialogs.create.variantWeightLabel")}
                        </label>
                        <div className="relative">
                          <Input
                            id={`create-variant-weight-${i}`}
                            type="number"
                            min={1}
                            max={100}
                            step={1}
                            className="pr-7"
                            value={row.weight}
                            onChange={(e) => updateVariantRow(i, { weight: e.target.value })}
                          />
                          <span className="pointer-events-none absolute inset-y-0 right-2.5 flex items-center text-sm text-muted-foreground">
                            %
                          </span>
                        </div>
                      </div>
                      <Button
                        type="button"
                        variant="ghost"
                        size="icon-sm"
                        aria-label={t("dialogs.create.removeVariant")}
                        onClick={() => removeVariantRow(i)}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    </div>
                  ))}

                  {variantRows.length > 0 && (
                    <div className="flex items-center justify-between gap-2">
                      <span
                        className={
                          variantsTotalValid
                            ? "text-sm font-medium text-muted-foreground"
                            : "text-sm font-medium text-destructive"
                        }
                      >
                        {t("dialogs.create.variantsTotal", { total: variantsTotal })}
                      </span>
                      <Button type="button" variant="ghost" size="sm" onClick={distributeVariantsEvenly}>
                        {t("dialogs.create.distributeEvenly")}
                      </Button>
                    </div>
                  )}

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
                    {t("dialogs.create.addVariant")}
                  </Button>
                </div>
              )}
            </div>

            <RulesEditor idPrefix="create-link" drafts={ruleDrafts} onChange={setRuleDrafts} />
            {errors.rules && (
              <p className="text-sm text-destructive" role="alert">
                {errors.rules}
              </p>
            )}

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
            <Button type="submit" disabled={createLink.isPending}>
              {createLink.isPending ? t("dialogs.create.submitting") : t("dialogs.create.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
