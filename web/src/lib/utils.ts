import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

/** Field-caption style for form labels across the app (13px muted, see `createOpen.html`). */
export const FIELD_LABEL_CLASS = "text-[13px] font-normal text-muted-foreground"
