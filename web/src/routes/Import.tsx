import { Inbox, Upload } from "lucide-react";
import { useRef, useState, type ChangeEvent, type FormEvent } from "react";
import { toast } from "sonner";
import { Button } from "@/components/ui/button";
import { Card, CardContent } from "@/components/ui/card";
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from "@/components/ui/table";
import { Textarea } from "@/components/ui/textarea";
import { useT, type MessageKey } from "@/i18n";
import { ApiError } from "@/lib/api";
import { isUnauthorized } from "@/lib/mutation-error";
import { useImport } from "@/lib/queries";
import type { ImportSummary } from "@/lib/types";

/** Friendly error message for the import mutation. */
function mutationErrorMessage(err: unknown, fallbackKey: MessageKey, t: (key: MessageKey) => string): string {
  if (err instanceof ApiError && err.status === 429) return t("common.rateLimited");
  return t(fallbackKey);
}

/**
 * Picks the `Content-Type` to send: a `.json` file name, or pasted text that
 * starts with `[` or `{`, is treated as JSON; everything else is CSV.
 */
function detectContentType(fileName: string | null, text: string): string {
  if (fileName?.toLowerCase().endsWith(".json")) return "application/json";
  if (fileName) return "text/csv";
  const trimmed = text.trim();
  return trimmed.startsWith("[") || trimmed.startsWith("{") ? "application/json" : "text/csv";
}

export function Import() {
  const t = useT();
  const fileInputRef = useRef<HTMLInputElement>(null);
  const [file, setFile] = useState<File | null>(null);
  const [text, setText] = useState("");
  const [inputError, setInputError] = useState<string | null>(null);
  const [summary, setSummary] = useState<ImportSummary | null>(null);
  const importMutation = useImport();

  function handleFileChange(e: ChangeEvent<HTMLInputElement>) {
    const picked = e.target.files?.[0] ?? null;
    setFile(picked);
    if (picked) setText("");
  }

  function handleTextChange(e: ChangeEvent<HTMLTextAreaElement>) {
    setText(e.target.value);
    if (e.target.value) {
      setFile(null);
      if (fileInputRef.current) fileInputRef.current.value = "";
    }
  }

  async function handleSubmit(e: FormEvent) {
    e.preventDefault();
    setInputError(null);

    const body = file ? await file.text() : text;
    if (!body.trim()) {
      setInputError(t("import.noInputError"));
      return;
    }
    const contentType = detectContentType(file?.name ?? null, body);

    try {
      const result = await importMutation.mutateAsync({ body, contentType });
      setSummary(result);
      toast.success(t("import.successToast", { imported: result.imported, failed: result.failed.length }));
    } catch (err) {
      if (isUnauthorized(err)) return;
      toast.error(mutationErrorMessage(err, "import.genericError", t));
    }
  }

  const hasInput = file != null || text.trim() !== "";

  return (
    <div className="flex flex-col gap-4">
      <div>
        <h1 className="font-heading text-2xl font-semibold">{t("import.heading")}</h1>
        <p className="mt-1 text-sm text-muted-foreground">{t("import.subtitle")}</p>
      </div>

      <Card>
        <CardContent>
          <form onSubmit={handleSubmit} className="flex flex-col gap-4">
            <div className="flex flex-col gap-1.5">
              <label htmlFor="import-file" className="text-sm font-medium">
                {t("import.fileLabel")}
              </label>
              <input
                id="import-file"
                ref={fileInputRef}
                type="file"
                accept=".csv,.json"
                aria-label={t("import.fileAriaLabel")}
                onChange={handleFileChange}
                className="text-sm file:mr-3 file:rounded-md file:border-0 file:bg-secondary file:px-3 file:py-1.5 file:text-sm file:font-medium file:text-secondary-foreground"
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor="import-textarea" className="text-sm font-medium">
                {t("import.pasteLabel")}
              </label>
              <Textarea
                id="import-textarea"
                placeholder={t("import.textareaPlaceholder")}
                value={text}
                onChange={handleTextChange}
                className="font-mono text-sm"
                rows={6}
              />
            </div>

            {inputError && <p className="text-sm text-destructive">{inputError}</p>}

            <div>
              <Button type="submit" disabled={importMutation.isPending || !hasInput}>
                <Upload className="size-4" />
                {importMutation.isPending ? t("import.submitting") : t("import.submit")}
              </Button>
            </div>
          </form>
        </CardContent>
      </Card>

      {!summary && (
        <Card>
          <CardContent className="flex flex-col items-center gap-3 py-12 text-center">
            <Inbox className="size-8 text-muted-foreground" aria-hidden="true" />
            <div>
              <p className="font-medium">{t("import.emptyTitle")}</p>
              <p className="text-sm text-muted-foreground">{t("import.emptySubtitle")}</p>
            </div>
          </CardContent>
        </Card>
      )}

      {summary && (
        <div className="flex flex-col gap-3">
          <p className="text-sm font-medium">
            {t("import.summary", { imported: summary.imported, failed: summary.failed.length })}
          </p>

          {summary.failed.length > 0 && (
            <Card className="py-0">
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead>{t("import.tableIndexHeader")}</TableHead>
                    <TableHead>{t("import.tableUrlHeader")}</TableHead>
                    <TableHead>{t("import.tableReasonHeader")}</TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {summary.failed.map((row) => (
                    <TableRow key={row.index}>
                      <TableCell>{row.index}</TableCell>
                      <TableCell className="max-w-xs truncate font-mono text-xs">{row.url}</TableCell>
                      <TableCell>{row.reason}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
            </Card>
          )}
        </div>
      )}
    </div>
  );
}
