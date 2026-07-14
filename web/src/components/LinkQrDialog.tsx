import { useRef } from "react";
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
import { useT } from "@/i18n";

const EXPORT_PIXEL_SIZE = 512;

interface LinkQrDialogProps {
  code: string;
  url: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Dialog with the short link's QR code, with a PNG download button. Uses
 * QRCodeSVG (instead of QRCodeCanvas) because SVG renders without `canvas`,
 * which keeps it testable in jsdom; the exported PNG is generated on demand,
 * only on the "Download" click, by drawing this SVG onto a temporary canvas —
 * we don't need canvas to display it, only to export it.
 */
export function LinkQrDialog({ code, url, open, onOpenChange }: LinkQrDialogProps) {
  const t = useT();
  const svgRef = useRef<SVGSVGElement>(null);

  function handleDownload() {
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
        ctx.fillStyle = "#ffffff";
        ctx.fillRect(0, 0, canvas.width, canvas.height);
        ctx.drawImage(image, 0, 0, canvas.width, canvas.height);

        const link = document.createElement("a");
        link.href = canvas.toDataURL("image/png");
        link.download = `quark-${code}.png`;
        link.click();
      }
      URL.revokeObjectURL(svgBlobUrl);
    };
    image.src = svgBlobUrl;
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
            <QRCodeSVG ref={svgRef} value={url} size={160} level="M" title={t("dialogs.qr.imageTitle", { url })} />
          </div>
          <p className="w-full truncate rounded-md bg-muted px-3 py-2 text-center font-mono text-sm" title={url}>
            {url}
          </p>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            {t("common.cancel")}
          </Button>
          <Button type="button" onClick={handleDownload}>
            {t("dialogs.qr.download")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
