/** One usage window as `status.json` has it, its reset time already worded by claude-subs. */
export type SubsWindow = {
  label: string
  percent: number
  resets_at: number | null
  /** "in 2h 05m", "tomorrow 11:00", "Sat 11:00". */
  resets: string | null
  /** "today 13:05", "Sat 7 Nov, 11:00". */
  resets_exact: string | null
}

export type SubsAccount = {
  id: string
  email: string
  /** The email's local part ("side"), or the whole email when two accounts share it. */
  short: string
  name: string | null
  org: string | null
  plan: string | null
  active: boolean
  status: 'ok' | 'needs_login'
  error: string | null
  session: SubsWindow | null
  week: SubsWindow | null
  scoped: SubsWindow[]
  usage_at: number | null
  usage_source: string | null
  login_expires_at: number | null
}

/** `status.json` in claude-subs' data folder: written by the tray app or by each command. */
export type SubsStatus = {
  format: number
  app_version: string
  /** `tray`: the Windows tray app keeps it fresh; `command`: the mod's own runs do. */
  driver: 'tray' | 'command'
  /** The binary that wrote it: what the mod runs. */
  exe: string | null
  active: string | null
  accounts: SubsAccount[]
  notice: string | null
  at: number
}

declare module 'claude-code' {
  interface PluginState {
    'subs-switcher': {
      status: Shaped<SubsStatus | null>
      /** The short name of the account a switch is under way to. */
      busy: string | null
      hidden: boolean
      placement: 'below' | 'above'
      /** Which page of the other accounts the band shows, from 0. */
      page: number
    }
  }
}
