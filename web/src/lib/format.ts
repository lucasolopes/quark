// Formatação de datas compartilhada pelas telas de Links e Estatísticas.
// Epoch em SEGUNDOS (como devolvido pela API) — convertido pra milissegundos
// antes de repassar ao Date/Intl. `0`/`null`/`undefined` significam "sem
// valor" (não epoch zero de verdade) nas respostas da API, por isso o guard.

/** Data curta (dia/mês/ano), pt-BR. `formatDate(0)` → "—". */
export function formatDate(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleDateString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
  });
}

/** Data e hora (dia/mês/ano hora:minuto), pt-BR. `formatDateTime(0)` → "—". */
export function formatDateTime(epochSeconds: number): string {
  if (!epochSeconds) return "—";
  return new Date(epochSeconds * 1000).toLocaleString("pt-BR", {
    day: "2-digit",
    month: "2-digit",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}
