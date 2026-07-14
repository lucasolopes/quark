/**
 * Parses a comma-separated tags input into an array. Splits on commas, trims
 * each entry, and drops empty ones — the server does its own normalization
 * (lowercase/dedupe/cap) at the API boundary, so this only handles turning
 * the raw text field into an array.
 */
export function parseTagsInput(input: string): string[] {
  return input
    .split(",")
    .map((tag) => tag.trim())
    .filter((tag) => tag.length > 0);
}

/** Joins tags back into the comma-separated form used by the tags input field. */
export function formatTagsInput(tags: string[]): string {
  return tags.join(", ");
}
