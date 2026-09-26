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

export function confidenceWord(c: number): string {
  if (c >= 0.9) return "certain";
  if (c >= 0.75) return "confident";
  if (c >= 0.55) return "fairly sure";
  return "uncertain";
}
