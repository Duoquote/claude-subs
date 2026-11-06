import * as React from "react"
import { ArrowsLeftRight, Trash, Warning } from "@phosphor-icons/react"
import { api, errorText } from "@/lib/api"
import { ago, cn, daysUntil, tone } from "@/lib/format"
import type { Account, Window } from "@/lib/types"

const fill = { ok: "bg-success", warn: "bg-warning", danger: "bg-destructive" } as const

function Meter({ w, label }: { w: Window | null; label: string }) {
  const p = w?.percent ?? null
  return (
    <div
      className="grid grid-cols-[64px_1fr_34px_100px] items-center gap-2 text-xs"
      title={w?.resets_exact ? `${label}: resets ${w.resets_exact}` : undefined}
    >
      <span className="truncate text-muted-foreground">{label}</span>
      <div className="h-1.5 overflow-hidden bg-muted" role="meter" aria-label={label} aria-valuenow={p ?? 0}>
        {p !== null && (
          <div className={cn("h-full", fill[tone(p)])} style={{ width: `${Math.min(100, Math.max(p > 0 ? 2 : 0, p))}%` }} />
        )}
      </div>
      <span className={cn("text-right tabular-nums", p !== null && p >= 85 && "font-medium text-destructive")}>
        {p === null ? "–" : `${Math.round(p)}%`}
      </span>
      <span className="truncate text-right text-[11px] text-muted-foreground tabular-nums">{w?.resets ?? ""}</span>
    </div>
  )
}

export function AccountRow({
  a,
  now,
  busy,
  onSwitch,
}: {
  a: Account
  now: number
  busy: boolean
  onSwitch: (id: string) => void
}) {
  const [confirm, setConfirm] = React.useState(false)
  const [error, setError] = React.useState<string | null>(null)
  React.useEffect(() => {
    if (!confirm) return
    const t = setTimeout(() => setConfirm(false), 3000)
    return () => clearTimeout(t)
  }, [confirm])

  const needsLogin = a.status === "needs_login"
  const expiresIn = a.login_expires_at ? daysUntil(a.login_expires_at, now) : null
  const subtitle = [a.org, a.plan].filter(Boolean).join(" · ")
  const scoped = a.scoped.filter((w) => w.percent > 0)
  const source = a.usage_at
    ? `${a.usage_source === "claude" ? "from Claude Code, " : ""}${ago(a.usage_at, now)}`
    : "no reading yet"

  const forget = async () => {
    if (!confirm) {
      setConfirm(true)
      return
    }
    try {
      await api.forget(a.id)
    } catch (e) {
      setError(errorText(e))
    }
  }

  return (
    <section
      aria-label={a.email}
      className={cn("flex flex-col gap-2 border-b border-border px-3 py-3", a.active && "bg-accent/60")}
    >
      <header className="flex items-center gap-2">
        <span
          aria-hidden
          className={cn(
            "inline-block size-2 shrink-0 rounded-full",
            a.active ? "bg-success" : needsLogin ? "bg-destructive" : "bg-muted-foreground/40"
          )}
        />
        <div className="min-w-0 flex-1">
          <div className="truncate text-sm font-medium" title={a.name ?? a.email}>
            {a.email}
          </div>
          {subtitle && <div className="truncate text-[11px] text-muted-foreground">{subtitle}</div>}
        </div>
        {a.active ? (
          <span className="text-[11px] font-medium text-success">Live</span>
        ) : (
          <button
            type="button"
            disabled={busy || needsLogin}
            onClick={() => onSwitch(a.id)}
            className="inline-flex h-7 items-center gap-1.5 border border-border bg-background px-2.5 text-xs font-medium hover:bg-accent disabled:cursor-not-allowed disabled:opacity-50"
          >
            <ArrowsLeftRight className="size-3.5" />
            Switch
          </button>
        )}
      </header>

      <Meter w={a.session} label="Session" />
      <Meter w={a.week} label="Week" />
      {scoped.map((w) => (
        <Meter key={w.label} w={w} label={w.label} />
      ))}

      <footer className="flex items-center gap-2 text-[11px] text-muted-foreground">
        <span className="min-w-0 flex-1 truncate">{source}</span>
        {expiresIn !== null && expiresIn <= 7 && !needsLogin && (
          <span className="inline-flex items-center gap-1 text-warning" title="Unused logins expire; a switch or a refresh renews it">
            <Warning className="size-3" />
            login expires in {Math.max(0, expiresIn)} d
          </span>
        )}
        {!a.active && (
          <button
            type="button"
            onClick={forget}
            className={cn("inline-flex items-center gap-1 hover:text-destructive", confirm && "text-destructive")}
            title="Remove this saved login from Claude Subs"
          >
            <Trash className="size-3" />
            {confirm ? "Click again to forget" : "Forget"}
          </button>
        )}
      </footer>
      {(a.error || error) && <p className="text-[11px] text-destructive">{error ?? a.error}</p>}
    </section>
  )
}
