import { useId, useRef, useState, type ComponentProps } from "react";
import { QRCodeSVG } from "qrcode.react";
import { Button } from "@/components/ui/button";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { cn } from "@/lib/utils";
import { useT } from "@/i18n";

const EXPORT_PIXEL_SIZE = 512;
const QR_MARGIN_SIZE = 2;

type ErrorCorrectionLevel = NonNullable<ComponentProps<typeof QRCodeSVG>["level"]>;

const ERROR_CORRECTION_LEVELS: ErrorCorrectionLevel[] = ["L", "M", "Q", "H"];

const DEFAULT_LEVEL: ErrorCorrectionLevel = "M";
const DEFAULT_FG_COLOR = "#0A0B0F";
const DEFAULT_BG_COLOR = "#FFFFFF";

interface LinkQrDialogProps {
  code: string;
  url: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

function triggerDownload(href: string, filename: string) {
  const link = document.createElement("a");
  link.href = href;
  link.download = filename;
  link.click();
}

/**
 * Dialog with the short link's QR code, with PNG and SVG download buttons and
 * controls for the error-correction level and the foreground/background
 * colors. Uses QRCodeSVG (instead of QRCodeCanvas) because SVG renders
 * without `canvas`, which keeps it testable in jsdom; the exported PNG is
 * generated on demand, only on the "Download PNG" click, by drawing this SVG
 * onto a temporary canvas — we don't need canvas to display it, only to
 * export it. The SVG download serializes the live SVG directly, no canvas
 * needed.
 */
export function LinkQrDialog({ code, url, open, onOpenChange }: LinkQrDialogProps) {
  const t = useT();
  const svgRef = useRef<SVGSVGElement>(null);
  const levelSelectId = useId();
  const fgColorId = useId();
  const bgColorId = useId();

  const [level, setLevel] = useState<ErrorCorrectionLevel>(DEFAULT_LEVEL);
  const [fgColor, setFgColor] = useState(DEFAULT_FG_COLOR);
  const [bgColor, setBgColor] = useState(DEFAULT_BG_COLOR);

  const levelLabels: Record<ErrorCorrectionLevel, string> = {
    L: t("dialogs.qr.levelL"),
    M: t("dialogs.qr.levelM"),
    Q: t("dialogs.qr.levelQ"),
    H: t("dialogs.qr.levelH"),
  };

  function handleDownloadPng() {
    const svg = svgRef.current;
    if (!svg) return;

    const svgString = new XMLSerializer().serializeToString(svg);
    const svgBlobUrl = URL.createObjectURL(new Blob([svgString], { type: "image/svg+xml;charset=utf-8" }));

    const image = new Image();
    image.onload = () => {
      const canvas = document.createElement("canvas");
      canvas.width = EXPORT_PIXEL_SIZE;
      canvas.height = EXPORT_PIXEL_SIZE;
      const ctx = canvas.getContext("2d");
      if (ctx) {
        ctx.fillStyle = bgColor;
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
        triggerDownload(canvas.toDataURL("image/png"), `quark-${code}.png`);
      }
      URL.revokeObjectURL(svgBlobUrl);
    };
    image.src = svgBlobUrl;
  }

  function handleDownloadSvg() {
    const svg = svgRef.current;
    if (!svg) return;

    const svgString = new XMLSerializer().serializeToString(svg);
    const svgBlobUrl = URL.createObjectURL(new Blob([svgString], { type: "image/svg+xml;charset=utf-8" }));
    triggerDownload(svgBlobUrl, `quark-${code}.svg`);
    URL.revokeObjectURL(svgBlobUrl);
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>{t("dialogs.qr.title", { code })}</DialogTitle>
          <DialogDescription>{t("dialogs.qr.description")}</DialogDescription>
        </DialogHeader>

        <div className="flex flex-col items-center gap-4 py-2">
          <div className="rounded-lg border bg-white p-4">
            <QRCodeSVG
              ref={svgRef}
              value={url}
              size={160}
              level={level}
              fgColor={fgColor}
              bgColor={bgColor}
              marginSize={QR_MARGIN_SIZE}
              title={t("dialogs.qr.imageTitle", { url })}
            />
          </div>
          <p className="w-full truncate rounded-md bg-muted px-3 py-2 text-center font-mono text-sm" title={url}>
            {url}
          </p>

          <div className="grid w-full grid-cols-2 gap-3">
            <div className="col-span-2 flex flex-col gap-1.5">
              <label htmlFor={levelSelectId} className="text-sm font-medium">
                {t("dialogs.qr.levelLabel")}
              </label>
              <select
                id={levelSelectId}
                value={level}
                onChange={(event) => setLevel(event.target.value as ErrorCorrectionLevel)}
                className={cn(
                  "h-8 w-full rounded-lg border border-input bg-transparent px-2.5 py-1 text-sm outline-none transition-colors",
                  "focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50",
                  "dark:bg-input/30",
                )}
              >
                {ERROR_CORRECTION_LEVELS.map((levelOption) => (
                  <option key={levelOption} value={levelOption}>
                    {levelLabels[levelOption]}
                  </option>
                ))}
              </select>
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor={fgColorId} className="text-sm font-medium">
                {t("dialogs.qr.fgColorLabel")}
              </label>
              <input
                id={fgColorId}
                type="color"
                value={fgColor}
                onChange={(event) => setFgColor(event.target.value)}
                className="h-8 w-full cursor-pointer rounded-lg border border-input bg-transparent p-1"
              />
            </div>

            <div className="flex flex-col gap-1.5">
              <label htmlFor={bgColorId} className="text-sm font-medium">
                {t("dialogs.qr.bgColorLabel")}
              </label>
              <input
                id={bgColorId}
                type="color"
                value={bgColor}
                onChange={(event) => setBgColor(event.target.value)}
                className="h-8 w-full cursor-pointer rounded-lg border border-input bg-transparent p-1"
              />
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button type="button" variant="outline" onClick={handleDownloadSvg}>
            {t("dialogs.qr.downloadSvg")}
          </Button>
          <Button type="button" onClick={handleDownloadPng}>
            {t("dialogs.qr.download")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
