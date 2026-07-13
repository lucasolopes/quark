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

// Tamanho de renderização do PNG exportado — maior que o QR em tela (160px)
// pra continuar legível quando impresso ou colado em outro material.
const EXPORT_PIXEL_SIZE = 512;

interface LinkQrDialogProps {
  code: string;
  url: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/**
 * Dialog com o QR code da URL curta de um link, com botão de download em
 * PNG. Usa QRCodeSVG (em vez do QRCodeCanvas) porque o SVG renderiza sem
 * `canvas`, o que o mantém testável em jsdom; o PNG exportado é gerado sob
 * demanda, só no clique de "Baixar", desenhando esse SVG num canvas
 * temporário — não precisamos de canvas pra exibir, só pra exportar.
 */
export function LinkQrDialog({ code, url, open, onOpenChange }: LinkQrDialogProps) {
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
      // jsdom (ambiente de teste) não implementa `getContext` — em produção,
      // todo navegador com suporte a canvas 2d chega aqui com `ctx` presente.
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
          <DialogTitle>QR code de {code}</DialogTitle>
          <DialogDescription>Aponte a câmera do celular pra abrir o link curto.</DialogDescription>
        </DialogHeader>

        <div className="flex flex-col items-center gap-4 py-2">
          <div className="rounded-lg border bg-white p-4">
            <QRCodeSVG ref={svgRef} value={url} size={160} level="M" title={`QR code de ${url}`} />
          </div>
          <p className="w-full truncate rounded-md bg-muted px-3 py-2 text-center font-mono text-sm" title={url}>
            {url}
          </p>
        </div>

        <DialogFooter>
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancelar
          </Button>
          <Button type="button" onClick={handleDownload}>
            Baixar PNG
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
