import { Loader2, Plus } from "lucide-react";
import { useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useCreateWorkspace } from "@/lib/queries";
import { FIELD_LABEL_CLASS } from "@/lib/utils";

/** Lowercases, strips accents, and turns runs of non-alphanumerics into single dashes. */
function slugify(input: string): string {
  return input
    .normalize("NFD")
    .replace(/[\u0300-\u036f]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/**
 * How long a create may run before the form admits it is slow.
 *
 * `POST /admin/tenants` writes the tenant and the membership and then
 * provisions the workspace's sign-in in Keycloak: realm, client, mapper and
 * user, each one an HTTP call with retry. A healthy run lands well inside this
 * window, so the notice stays out of the way of the normal case and only shows
 * up when something really is dragging. A starting value, meant to be revised
 * against measured provisioning times rather than treated as a constant of
 * nature.
 */
export const SLOW_CREATE_NOTICE_MS = 8_000;

/** Name+slug form to create a workspace. `onCreated` fires after a successful create. */
export function CreateWorkspaceForm({ onCreated }: { onCreated?: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugEdited, setSlugEdited] = useState(false);
  const [slow, setSlow] = useState(false);
  const mutation = useCreateWorkspace();

  // A single timer, not a sequence of fabricated steps. The panel does no
  // polling, so it has no idea which stage the server is on; the only honest
  // thing it can add over time is "this is running long".
  const pending = mutation.isPending;
  useEffect(() => {
    if (!pending) { setSlow(false); return; }
    const id = setTimeout(() => setSlow(true), SLOW_CREATE_NOTICE_MS);
    return () => clearTimeout(id);
  }, [pending]);

  const effectiveSlug = slugEdited ? slug : slugify(name);

  function handleSubmit(e: React.FormEvent<HTMLFormElement>) {
    e.preventDefault();
    if (!name.trim() || !effectiveSlug || mutation.isPending) return;
    mutation.mutate(
      { name: name.trim(), slug: effectiveSlug },
      { onSuccess: () => onCreated?.() },
    );
  }

  const errorText =
    mutation.error instanceof ApiError && mutation.error.status === 409
      ? t("onboarding.slugTaken")
      : mutation.error instanceof ApiError && mutation.error.status === 429
        ? t("common.rateLimited")
        : mutation.isError
          ? t("onboarding.createError")
          : null;

  return (
    <form onSubmit={handleSubmit} className="flex flex-col gap-3" noValidate>
      <div className="flex flex-col gap-1.5">
        <label htmlFor="ws-name" className={FIELD_LABEL_CLASS}>{t("onboarding.nameLabel")}</label>
        <Input
          id="ws-name"
          value={name}
          placeholder={t("onboarding.namePlaceholder")}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
      </div>
      <div className="flex flex-col gap-1.5">
        <label htmlFor="ws-slug" className={FIELD_LABEL_CLASS}>{t("onboarding.slugLabel")}</label>
        <Input
          id="ws-slug"
          value={effectiveSlug}
          onChange={(e) => { setSlugEdited(true); setSlug(slugify(e.target.value)); }}
          className="font-mono"
          aria-invalid={mutation.isError}
          aria-describedby={mutation.isError ? "ws-slug-error" : undefined}
        />
        <p className="text-xs text-muted-foreground">{t("onboarding.slugHint")}</p>
      </div>
      {errorText && <p id="ws-slug-error" role="alert" className="text-sm text-destructive">{errorText}</p>}
      {pending && (
        <div aria-live="polite" className="flex flex-col gap-1">
          <p className="text-xs text-muted-foreground">{t("onboarding.creatingDetail")}</p>
          {slow && <p className="text-xs text-muted-foreground">{t("onboarding.creatingSlow")}</p>}
        </div>
      )}
      <Button type="submit" disabled={!name.trim() || !effectiveSlug || mutation.isPending} className="mt-1">
        {mutation.isPending ? (
          <Loader2 className="size-4 animate-spin" aria-hidden="true" />
        ) : (
          <Plus className="size-4" aria-hidden="true" />
        )}
        {mutation.isPending ? t("onboarding.creating") : t("onboarding.submit")}
      </Button>
    </form>
  );
}
