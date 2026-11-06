// The engine's snapshot (app/src/engine.rs `Snapshot`), as the popover receives it.

export type Window = {
  label: string
  percent: number
  /** Unix ms. */
  resets_at: number | null
  /** Worded by the app (system 12/24-hour clock): "in 2h 05m", "tomorrow 11:00", "Sat 11:00". */
  resets: string | null
  /** "today 13:05", "Sat 7 Nov, 11:00": for hover. */
  resets_exact: string | null
}

export type Status = "ok" | "needs_login"

export type Account = {
  id: string
  email: string
  name: string | null
  org: string | null
  plan: string | null
  active: boolean
  status: Status
  error: string | null
  session: Window | null
  week: Window | null
  scoped: Window[]
  usage_at: number | null
  usage_source: "api" | "claude" | null
  login_expires_at: number | null
}

export type Snapshot = {
  active: string | null
  accounts: Account[]
  notice: string | null
  at: number
}
