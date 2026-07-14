import { ChevronDown, ChevronRight, Trash2 } from "lucide-react";
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
import { applyUtm, deleteUtmTemplate, loadUtmTemplates, saveUtmTemplate, type UtmParams } from "@/lib/utm";

interface FormErrors {
  url?: string;
  alias?: string;
  ttl?: string;
  form?: string;
}

const EMPTY_UTM: UtmParams = {};

/** Whether at least one UTM field carries a non-empty value. */
function hasAnyUtm(params: UtmParams): boolean {
  return Object.values(params).some((value) => value != null && value.trim() !== "");
}

interface CreateLinkDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Short link creation dialog. Validates client-side (http/https URL, alias
 * outside the numeric-code space, positive TTL) before calling the API —
 * avoids a round-trip just to get back an error we already knew about.
 */
export function CreateLinkDialog({ open, onOpenChange }: CreateLinkDialogProps) {
  const t = useT();
  const [url, setUrl] = useState("");
  const [alias, setAlias] = useState("");
  const [ttl, setTtl] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
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
    setErrors({});
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
    const trimmedTtl = ttl.trim();
    if (trimmedTtl) {
      const n = Number(trimmedTtl);
      if (!Number.isInteger(n) || n <= 0) {
        next.ttl = t("dialogs.create.ttlInvalid");
      }
    }
    return next;
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault();
    const nextErrors = validate();
    if (Object.keys(nextErrors).length > 0) {
      setErrors(nextErrors);
      return;
    }
    setErrors({});
    const trimmedUrl = url.trim();
    const destination = hasAnyUtm(utm) ? applyUtm(trimmedUrl, utm) : trimmedUrl;
    try {
      await createLink.mutateAsync({
        url: destination,
        ...(alias.trim() ? { alias: alias.trim() } : {}),
        ...(ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
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
      <DialogContent>
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>{t("dialogs.create.title")}</DialogTitle>
            <DialogDescription>{t("dialogs.create.description")}</DialogDescription>
          </DialogHeader>

          <div className="flex flex-col gap-3 py-3">
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
              <label htmlFor="create-link-ttl" className="text-sm font-medium">
                {t("dialogs.create.ttlLabel")} <span className="text-muted-foreground">{t("dialogs.create.ttlOptional")}</span>
              </label>
              <Input
                id="create-link-ttl"
                type="number"
                min={1}
                step={1}
                placeholder={t("dialogs.create.ttlPlaceholder")}
                value={ttl}
                onChange={(e) => setTtl(e.target.value)}
                aria-invalid={errors.ttl != null}
              />
              {errors.ttl && (
                <p className="text-sm text-destructive" role="alert">
                  {errors.ttl}
                </p>
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
            <Button type="submit" disabled={createLink.isPending}>
              {createLink.isPending ? t("dialogs.create.submitting") : t("dialogs.create.submit")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  );
}
