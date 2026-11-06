// Reset times are worded by the app (app/src/when.rs), which follows the Windows 12/24-hour setting;
// WebView2's own locale does not.

/** "just now", "4 min ago", "2 h ago". */
export function ago(at: number, now: number): string {
  const mins = Math.floor((now - at) / 60_000)
  if (mins < 1) return "just now"
  if (mins < 60) return `${mins} min ago`
  const hours = Math.floor(mins / 60)
  if (hours < 48) return `${hours} h ago`
  return `${Math.floor(hours / 24)} days ago`
}

export function daysUntil(at: number, now: number): number {
  return Math.floor((at - now) / 86_400_000)
}

/** Status colour of a percentage: the tray icon uses the same thresholds. */
export function tone(percent: number): "ok" | "warn" | "danger" {
  if (percent >= 85) return "danger"
  if (percent >= 60) return "warn"
  return "ok"
}

export function cn(...parts: (string | false | null | undefined)[]): string {
  return parts.filter(Boolean).join(" ")
}
