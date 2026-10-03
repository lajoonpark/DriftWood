import type { Candidate } from "./types";

export function formatBytes(bytes: number, precision?: number): string {
  if (bytes === 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(
    Math.floor(Math.log(Math.abs(bytes)) / Math.log(1024)),
    units.length - 1,
  );
  const value = bytes / Math.pow(1024, i);
  const digits = precision ?? (i === 0 ? 0 : value >= 100 ? 0 : value >= 10 ? 1 : 2);
  return `${value.toFixed(digits)} ${units[i]}`;
}

export function formatCount(n: number): string {
  return new Intl.NumberFormat("en-US").format(Math.round(n));
}

export function truncateMiddle(path: string, max = 48): string {
  if (path.length <= max) return path;
  const head = Math.ceil((max - 1) / 2);
  const tail = Math.floor((max - 1) / 2);
  return path.slice(0, head) + "…" + path.slice(path.length - tail);
}

export function fileName(path: string): string {
  const parts = path.split("/");
  return parts[parts.length - 1] || path;
}

export function relDate(iso?: string): string {
  if (!iso) return "date unknown";
  const then = new Date(iso).getTime();
  const days = Math.floor((Date.now() - then) / 86_400_000);
  if (days <= 0) return "today";
  if (days === 1) return "yesterday";
  if (days < 31) return `${days} days ago`;
  const months = Math.round(days / 30.4);
  if (months < 18) return `${months} month${months === 1 ? "" : "s"} ago`;
  const years = +(days / 365).toFixed(1);
  return `${years} years ago`;
}

/** Why a tri-state field has no value, or null when it is known. */
export function fieldUnavailable(
  state?: Candidate["size_state"],
): string | null {
  if (!state || state === "known") return null;
  if (typeof state === "object" && "error" in state) return state.error;
  return "unavailable";
}

/** Size for display. An unreadable item shows the reason — never "0 B". */
export function sizeLabel(
  c: Pick<Candidate, "size_bytes" | "readable" | "size_state" | "read_error">,
): string {
  const state = c.size_state;
  const unreadable =
    c.readable === false || (state !== undefined && state !== "known");
  if (!unreadable) return formatBytes(c.size_bytes);
  const reason = c.read_error ?? fieldUnavailable(state) ?? "unreadable";
  return `not inspected (${reason})`;
}

/** Last-used is shown only when macOS Spotlight actually recorded a use.
 *  A filesystem mtime is NOT last-used, so when it is missing we say
 *  "unknown" rather than inferring activity from it. */
export function lastUsedLabel(
  c: Pick<Candidate, "last_used_date" | "last_used_from_spotlight">,
): string {
  if (c.last_used_from_spotlight && c.last_used_date) {
    return `last used ${relDate(c.last_used_date)}`;
  }
  return "last used unknown";
}

/** A filesystem date is labeled with the field it came from. */
export function modifiedLabel(
  c: Pick<Candidate, "modified_date">,
): string | null {
  return c.modified_date ? `modified ${relDate(c.modified_date)}` : null;
}

export function createdLabel(
  c: Pick<Candidate, "created_date">,
): string | null {
  return c.created_date ? `created ${relDate(c.created_date)}` : null;
}

export function confidenceWord(c: number): string {
  if (c >= 0.9) return "certain";
  if (c >= 0.75) return "confident";
  if (c >= 0.55) return "fairly sure";
  return "uncertain";
}
