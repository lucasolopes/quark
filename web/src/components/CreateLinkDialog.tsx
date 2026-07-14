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
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isHttpUrl, isNumericCode } from "@/lib/codeguard";
import { isUnauthorized } from "@/lib/mutation-error";
import { useCreateLink } from "@/lib/queries";

interface FormErrors {
  url?: string;
  alias?: string;
  ttl?: string;
  appIos?: string;
  appAndroid?: string;
  form?: string;
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
  const [appIos, setAppIos] = useState("");
  const [appAndroid, setAppAndroid] = useState("");
  const [errors, setErrors] = useState<FormErrors>({});
  const createLink = useCreateLink();

  function reset() {
    setUrl("");
    setAlias("");
    setTtl("");
    setAppIos("");
    setAppAndroid("");
    setErrors({});
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
    const trimmedTtl = ttl.trim();
    if (trimmedTtl) {
      const n = Number(trimmedTtl);
      if (!Number.isInteger(n) || n <= 0) {
        next.ttl = t("dialogs.create.ttlInvalid");
      }
    }
    if (appIos.trim() && !isHttpUrl(appIos)) {
      next.appIos = t("dialogs.create.appDestInvalid");
    }
    if (appAndroid.trim() && !isHttpUrl(appAndroid)) {
      next.appAndroid = t("dialogs.create.appDestInvalid");
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
    try {
      await createLink.mutateAsync({
        url: url.trim(),
        ...(alias.trim() ? { alias: alias.trim() } : {}),
        ...(ttl.trim() ? { ttl: Number(ttl.trim()) } : {}),
        ...(appIos.trim() ? { app_ios: appIos.trim() } : {}),
        ...(appAndroid.trim() ? { app_android: appAndroid.trim() } : {}),
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

            <div className="flex flex-col gap-1.5">
              <span className="text-sm font-medium">
                {t("dialogs.create.appDestLabel")} <span className="text-muted-foreground">{t("dialogs.create.optional")}</span>
              </span>
              <p className="text-sm text-muted-foreground">{t("dialogs.create.appDestNote")}</p>
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
