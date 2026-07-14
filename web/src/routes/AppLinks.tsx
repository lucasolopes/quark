import { Info, Save, Trash2 } from "lucide-react";
import { useEffect, useState } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardHeader, CardTitle } from "@/components/ui/card";
import { useT, type MessageKey } from "@/i18n";
import { mutationErrorToast } from "@/lib/mutation-error";
import type { WellknownName } from "@/lib/types";
import { useDeleteWellknown, usePutWellknown, useWellknown } from "@/lib/queries";
import { cn } from "@/lib/utils";

export function AppLinks() {
  const t = useT();

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="font-heading text-2xl font-semibold">{t("appLinks.heading")}</h1>
        <p className="mt-1 text-sm text-muted-foreground">{t("appLinks.subtitle")}</p>
      </div>

      <div className="flex items-start gap-2 rounded-lg border border-border bg-muted/40 px-3 py-2.5 text-sm text-muted-foreground">
        <Info className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
        <p>{t("appLinks.httpsNote")}</p>
      </div>

      <WellknownEditor name="apple-app-site-association" titleKey="appLinks.aasaTitle" descriptionKey="appLinks.aasaDescription" />
      <WellknownEditor name="assetlinks.json" titleKey="appLinks.assetlinksTitle" descriptionKey="appLinks.assetlinksDescription" />
    </div>
  );
}

interface WellknownEditorProps {
  name: WellknownName;
  titleKey: MessageKey;
  descriptionKey: MessageKey;
}

function WellknownEditor({ name, titleKey, descriptionKey }: WellknownEditorProps) {
  const t = useT();
  const query = useWellknown(name);
  const put = usePutWellknown(name);
  const del = useDeleteWellknown(name);
  const [draft, setDraft] = useState("");

  useEffect(() => {
    if (query.data != null) setDraft(query.data);
  }, [query.data]);

  const trimmed = draft.trim();
  const isEmpty = trimmed === "";
  let isValid = false;
  if (!isEmpty) {
    try {
      JSON.parse(trimmed);
      isValid = true;
    } catch {
      isValid = false;
    }
  }
  const showInvalid = !isEmpty && !isValid;
  const canSave = isValid && !put.isPending;

  async function handleSave() {
    try {
      await put.mutateAsync(trimmed);
      toast.success(t("appLinks.saveSuccess", { name }));
    } catch (err) {
      mutationErrorToast(err, () => t("appLinks.saveError"));
    }
  }

  async function handleClear() {
    try {
      await del.mutateAsync();
      setDraft("");
      toast.success(t("appLinks.clearSuccess", { name }));
    } catch (err) {
      mutationErrorToast(err, () => t("appLinks.clearError"));
    }
  }

  return (
    <Card role="region" aria-label={t(titleKey)}>
      <CardHeader>
        <CardTitle>{t(titleKey)}</CardTitle>
        <CardDescription>{t(descriptionKey)}</CardDescription>
      </CardHeader>
      <CardContent className="flex flex-col gap-3">
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          placeholder={t("appLinks.placeholder")}
          aria-label={t("appLinks.editorAria", { name })}
          aria-invalid={showInvalid}
          spellCheck={false}
          rows={10}
          className={cn(
            "w-full min-w-0 rounded-lg border border-input bg-transparent px-2.5 py-2 font-mono text-sm transition-colors outline-none placeholder:text-muted-foreground focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50 aria-invalid:border-destructive aria-invalid:ring-3 aria-invalid:ring-destructive/20 dark:bg-input/30",
          )}
        />

        {query.isError && <p className="text-sm text-destructive">{t("appLinks.loadError")}</p>}
        {showInvalid && <p className="text-sm text-destructive">{t("appLinks.invalidJson")}</p>}

        <div className="flex flex-wrap items-center gap-2">
          <Button type="button" onClick={handleSave} disabled={!canSave}>
            <Save className="size-4" />
            {put.isPending ? t("appLinks.saving") : t("appLinks.save")}
          </Button>
          <Button type="button" variant="outline" onClick={handleClear} disabled={del.isPending}>
            <Trash2 className="size-4" />
            {del.isPending ? t("appLinks.clearing") : t("appLinks.clear")}
          </Button>
        </div>
      </CardContent>
    </Card>
  );
}
