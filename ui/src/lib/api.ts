import * as React from "react"
import { invoke } from "@tauri-apps/api/core"
import { listen } from "@tauri-apps/api/event"
import type { Snapshot } from "./types"

export const api = {
  snapshot: () => invoke<Snapshot>("snapshot"),
  switchTo: (id: string) => invoke<Snapshot>("switch_account", { id }),
  forget: (id: string) => invoke<Snapshot>("forget_account", { id }),
  refresh: () => invoke<void>("refresh_now"),
  hide: () => invoke<void>("hide"),
}

export function errorText(e: unknown): string {
  if (typeof e === "string") return e
  if (e instanceof Error) return e.message
  return String(e)
}

/** The engine's snapshot: read once, then every `snapshot` event. */
export function useSnapshot(): Snapshot | null {
  const [snap, setSnap] = React.useState<Snapshot | null>(null)
  React.useEffect(() => {
    let alive = true
    const off = listen<Snapshot>("snapshot", (e) => alive && setSnap(e.payload))
    api.snapshot().then((s) => alive && setSnap(s), () => undefined)
    return () => {
      alive = false
      void off.then((f) => f())
    }
  }, [])
  return snap
}

/** Re-renders every `ms` (countdowns). */
export function useNow(ms: number): number {
  const [now, setNow] = React.useState(() => Date.now())
  React.useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms)
    return () => clearInterval(t)
  }, [ms])
  return now
}

/** Follows the system light/dark setting. */
export function useSystemTheme() {
  React.useEffect(() => {
    const q = window.matchMedia("(prefers-color-scheme: dark)")
    const apply = () => document.documentElement.classList.toggle("dark", q.matches)
    apply()
    q.addEventListener("change", apply)
    return () => q.removeEventListener("change", apply)
  }, [])
}
