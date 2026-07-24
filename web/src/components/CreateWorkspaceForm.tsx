import { Loader2, Plus } from "lucide-react";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { useT } from "@/i18n";
import { ApiError } from "@/lib/api";
import { useCreateWorkspace } from "@/lib/queries";

/** Field-caption style for this form's labels (13px muted, matches CreateTokenDialog/Tokens v2). */
const FIELD_LABEL_CLASS = "text-[13px] font-normal text-muted-foreground";

/** Lowercases, strips accents, and turns runs of non-alphanumerics into single dashes. */
function slugify(input: string): string {
  return input
    .normalize("NFD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** Name+slug form to create a workspace. `onCreated` fires after a successful create. */
export function CreateWorkspaceForm({ onCreated }: { onCreated?: () => void }) {
  const t = useT();
  const [name, setName] = useState("");
  const [slug, setSlug] = useState("");
  const [slugEdited, setSlugEdited] = useState(false);
  const mutation = useCreateWorkspace();

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
