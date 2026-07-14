import { toast } from "sonner";
import { ApiError } from "./api";

/**
 * `true` when `err` is the 401 returned by the API. The global handler
 * (`setUnauthorizedHandler`, see App.tsx) already clears the token and
 * redirects to `/login` in that case — mutations should not show their own
 * feedback (toast or form error), or the user would see a redundant message
 * right before the redirect.
 */
export function isUnauthorized(err: unknown): boolean {
  return err instanceof ApiError && err.status === 401;
}

/**
 * Shows an error toast for simple mutations (delete link, add/remove
 * blocklist) — except on 401, where the global handler already takes care
 * of the feedback. `mapMessage` maps the error to a friendly message
 * (403/429/etc; see callers for each mutation's specific cases).
 */
export function mutationErrorToast(err: unknown, mapMessage: (err: unknown) => string): void {
  if (isUnauthorized(err)) return;
  toast.error(mapMessage(err));
}
