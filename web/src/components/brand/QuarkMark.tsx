import { cn } from "@/lib/utils";

/**
 * Marca oficial do quark — o glifo "Feistel-crossing": quatro nós lime
 * (as metades L/R entrando e saindo), o X (a troca por round) e o anel
 * central (a round-function ARX). Codifica o próprio motor (permutação
 * reversível com chave). Desenhado em `currentColor` por padrão pra herdar
 * a cor do contexto; passe `className="text-primary"` pra lime.
 */
export function QuarkMark({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 48 48"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      role="img"
      aria-label="quark"
      className={cn("size-6", className)}
    >
      <g stroke="currentColor" strokeWidth="4" strokeLinecap="round" fill="none">
        <path d="M14 14 L34 34" />
        <path d="M34 14 L14 34" />
      </g>
      <circle cx="24" cy="24" r="7" fill="none" stroke="currentColor" strokeWidth="4" />
      <g fill="currentColor">
        <circle cx="14" cy="14" r="2.6" />
        <circle cx="34" cy="14" r="2.6" />
        <circle cx="14" cy="34" r="2.6" />
        <circle cx="34" cy="34" r="2.6" />
      </g>
    </svg>
  );
}
