import * as React from "react"
import { ArrowClockwise } from "@phosphor-icons/react"
import { AccountRow } from "@/components/account-row"
import { api, errorText, useNow, useSnapshot, useSystemTheme } from "@/lib/api"
import { cn } from "@/lib/format"

export default function App() {
  useSystemTheme()
  const snap = useSnapshot()
  const now = useNow(30_000)
  const [busy, setBusy] = React.useState<string | null>(null)
  const [error, setError] = React.useState<string | null>(null)
  const [spinning, setSpinning] = React.useState(false)

  React.useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") void api.hide()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])

  const switchTo = async (id: string) => {
    setBusy(id)
    setError(null)
    try {
      await api.switchTo(id)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(null)
    }
  }

  const refresh = () => {
    setSpinning(true)
    void api.refresh()
    setTimeout(() => setSpinning(false), 1200)
  }

  const accounts = snap?.accounts ?? []
  const live = accounts.find((a) => a.active)
  const notice = error ?? snap?.notice ?? null

  return (
    <div className="flex h-svh min-h-0 flex-col bg-background text-foreground">
      <header className="flex h-10 shrink-0 items-center gap-2 border-b border-border px-3">
        <span className="font-heading text-sm font-semibold tracking-tight">Claude accounts</span>
        <span className="min-w-0 flex-1 truncate text-[11px] text-muted-foreground">
          {busy ? "switching…" : live ? `${accounts.length} saved` : ""}
        </span>
        <button
          type="button"
          onClick={refresh}
          title="Read usage now"
          aria-label="Read usage now"
          className="inline-flex size-7 items-center justify-center hover:bg-accent"
        >
          <ArrowClockwise className={cn("size-4", spinning && "animate-spin")} />
        </button>
      </header>

      {notice && (
        <p role="alert" className="shrink-0 border-b border-border bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {notice}
        </p>
      )}

      <main className="no-scrollbar min-h-0 flex-1 overflow-y-auto">
        {snap === null ? null : accounts.length === 0 ? (
          <div className="flex flex-col gap-2 px-3 py-6 text-sm">
            <p className="font-medium">No saved accounts yet</p>
            <p className="text-xs text-muted-foreground">
              Log in with Claude Code (<code>claude</code>, then <code>/login</code>). Each account you log in with is
              saved here automatically.
            </p>
          </div>
        ) : (
          accounts.map((a) => (
            <AccountRow key={a.id} a={a} now={now} busy={busy !== null} onSwitch={(id) => void switchTo(id)} />
          ))
        )}
      </main>

      <footer className="shrink-0 border-t border-border px-3 py-2 text-[11px] leading-snug text-muted-foreground">
        Add an account: <code>/login</code> in any Claude Code window. Don't <code>/logout</code>: it revokes the
        saved login. Running Claude Code windows follow a switch.
      </footer>
    </div>
  )
}
