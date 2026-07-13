import { toast } from "sonner";
import { ApiError } from "./api";

/**
 * `true` quando `err` é o 401 devolvido pela API. O handler global
 * (`setUnauthorizedHandler`, ver App.tsx) já limpa o token e redireciona pro
 * `/login` nesse caso — mutações não devem mostrar feedback próprio (toast ou
 * erro de formulário), ou o usuário veria uma mensagem redundante bem antes
 * do redirecionamento.
 */
export function isUnauthorized(err: unknown): boolean {
  return err instanceof ApiError && err.status === 401;
}

/**
 * Mostra um toast de erro pra mutações simples (deletar link, add/remover
 * blocklist) — exceto em 401, onde o handler global já cuida do feedback.
 * `mapMessage` mapeia o erro pra uma mensagem amigável (403/429/etc; ver
 * chamadores para os casos específicos de cada mutação).
 */
export function mutationErrorToast(err: unknown, mapMessage: (err: unknown) => string): void {
  if (isUnauthorized(err)) return;
  toast.error(mapMessage(err));
}
