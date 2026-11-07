// Claude Subs in Claude Code: a band under the prompt (or above it). On its left the live account's
// usage; on its right one other saved account, the best one to switch to (the soonest weekly reset
// with room left), with a Switch button and ‹ › to page through the rest. A press switches the
// login, and every running session follows (the same way a /login in one session reaches the
// others).
//
// The accounts come from `status.json`, which the claude-subs binary writes (the Windows tray app,
// or each command this mod runs). Switching runs `claude-subs switch <id>`. Where no tray app
// keeps the readings fresh (Linux, macOS), the mod runs `claude-subs tick` itself: once the status is a
// minute and a half old, and when Claude Code's credentials file changes (a token refresh or a
// /login), so the latest tokens are always saved before anything can replace them.

import { atom, read, update } from 'claude-code'
import type { ElementTable, EngineInterface, Register } from 'claude-code'

import type { SubsAccount, SubsStatus, SubsWindow } from '../types'

type Engine = EngineInterface

const status = atom({ plugin: 'subs-switcher', key: 'status' } as const, null, { shape: 'status-1' })
const busy = atom({ plugin: 'subs-switcher', key: 'busy' } as const, null)
const hidden = atom({ plugin: 'subs-switcher', key: 'hidden' } as const, false)
/** Where the band draws: under the prompt (the hint line's row, the default) or above it. */
const placement = atom({ plugin: 'subs-switcher', key: 'placement' } as const, 'below')
/** Where ‹ › left the band, until it goes back to the best account. */
const browse = atom({ plugin: 'subs-switcher', key: 'browse' } as const, null)

/** Where claude-subs and Claude Code keep their files on this machine. */
type Where = {
  windows: boolean
  statusFile: string
  credentials: string
  /** The binary beside an installed mod (`<data dir>/bin`), when no status names one. */
  installedExe: string
}

let where: Where | null = null
let statusMtime: number | null = null
let credentialsMtime: number | null = null
let lastTick = 0
let ticking = false
let logoutAskedAt: number | null = null

async function locate($: Engine): Promise<Where> {
  const windows = (await $.env.get('OS')) === 'Windows_NT'
  const join = (...parts: string[]) => parts.join(windows ? '\\' : '/')
  const home = (windows ? await $.env.get('USERPROFILE') : await $.env.get('HOME')) ?? ''
  let dataDir = await $.env.get('CLAUDE_SUBS_DATA_DIR')
  if (!dataDir && windows) {
    const local = await $.env.get('LOCALAPPDATA')
    dataDir = local ? join(local, 'claude-subs') : join(home, '.claude-subs')
  }
  if (!dataDir) {
    const xdg = await $.env.get('XDG_DATA_HOME')
    dataDir = join(xdg || join(home, '.local', 'share'), 'claude-subs')
  }
  const claudeDir = (await $.env.get('CLAUDE_CONFIG_DIR')) || join(home, '.claude')
  return {
    windows,
    statusFile: join(dataDir, 'status.json'),
    credentials: join(claudeDir, '.credentials.json'),
    installedExe: join($.plugin.root, '..', 'bin', windows ? 'claude-subs.exe' : 'claude-subs'),
  }
}

/** Reads `status.json` again when it changed (or with `force`), into the `status` atom. */
async function load($: Engine, force = false): Promise<SubsStatus | null> {
  if (!where) {
    return null
  }
  let mtime: number
  try {
    mtime = (await $.fs.stat(where.statusFile)).mtimeMs
  } catch {
    if (statusMtime !== -1) {
      statusMtime = -1
      await update($, status, () => null)
    }
    return null
  }
  if (!force && mtime === statusMtime) {
    return read($, status)
  }
  try {
    const parsed = JSON.parse(await $.fs.read(where.statusFile)) as SubsStatus
    statusMtime = mtime
    await update($, status, () => parsed)
    return parsed
  } catch {
    return read($, status)
  }
}

type Ran = { ok: boolean; out: string; err: string }

/** Runs claude-subs with `args`: the binary the status names, else the installed one, else PATH's. */
async function subs($: Engine, args: string[], timeoutMs = 60_000): Promise<Ran> {
  const named = (await read($, status))?.exe
  const candidates = [named, where?.installedExe, 'claude-subs'].filter(
    (c, i, all): c is string => !!c && all.indexOf(c) === i,
  )
  let failure = 'claude-subs was not found'
  for (const exe of candidates) {
    try {
      const r = await $.process.run([exe, ...args], { timeoutMs })
      return { ok: r.exitCode === 0, out: r.stdout.trimEnd(), err: r.stderr.trim().replace(/^claude-subs: /, '') }
    } catch (e) {
      failure = e instanceof Error ? e.message : String(e)
    }
  }
  return { ok: false, out: '', err: failure }
}

async function tick($: Engine) {
  if (ticking) {
    return
  }
  ticking = true
  try {
    lastTick = await $.clock.now()
    await subs($, ['tick'])
    await load($, true)
  } finally {
    ticking = false
  }
}

/**
 * Every few seconds: bring the band back to the best account once ‹ › has rested; pick up a new
 * status; where no tray app runs, keep the readings going.
 */
async function watch($: Engine) {
  const now = await $.clock.now()
  const was = await read($, browse)
  if (was !== null && now - was.at >= SNAP_MS) {
    await update($, browse, () => null)
  }
  if (!where) {
    return
  }
  const st = await load($)
  if (st?.driver === 'tray' && now - st.at < 150_000) {
    return
  }
  let mtime: number | null = null
  try {
    mtime = (await $.fs.stat(where.credentials)).mtimeMs
  } catch {
    // Logged out: nothing to save.
  }
  const changed = credentialsMtime !== null && mtime !== credentialsMtime
  credentialsMtime = mtime
  const stale = !st || now - st.at > 90_000
  if (changed || (stale && now - lastTick > 60_000)) {
    await tick($)
  }
}

async function switchTo($: Engine, a: SubsAccount) {
  if ((await read($, busy)) !== null) {
    return
  }
  await update($, busy, () => a.short)
  let r: Ran
  try {
    r = await subs($, ['switch', a.id], 120_000)
  } finally {
    await update($, busy, () => null)
  }
  await load($, true)
  if (r.ok) {
    $.ui.toast(`Switched to ${a.email}`)
    // The others are now another list: back to the best of them.
    await update($, browse, () => null)
    // The new account's first reading.
    void tick($)
  } else {
    $.ui.toast(`Switch failed: ${r.err.split('\n')[0] || 'claude-subs did not answer'}`, { timeoutMs: 8_000 })
  }
}

// ---- Which account the band offers ------------------------------------------------------------

/** A week, in hours: the furthest a reset can be. */
const WEEK_H = 168
/** Less room than this (% of the week, or of a running session) is not worth a switch. */
const MIN_ROOM = 10
/** How long the band stays where ‹ › left it before it shows the best account again. */
const SNAP_MS = 30_000

/** An account's 5-hour session: no such limit on its plan, not started yet, or under way. */
type Session = 'none' | 'idle' | 'running'
const sessionOf = (a: SubsAccount): Session =>
  a.session === null ? 'none' : a.session.resets_at === null ? 'idle' : 'running'

type Rated = { account: SubsAccount; score: number; room: number; usable: boolean }

/**
 * How good a pick an account is now. Weekly room that resets unused is lost, so the account whose
 * week resets soonest scores highest (0 a week away, 100 resetting now), plus up to 20 for a
 * running session with room left that resets soon. Under 10% of room it is not usable, unless all
 * that holds it back is a session that frees up within 15 minutes.
 */
function rate(a: SubsAccount, now: number): Rated {
  const hours = (w: SubsWindow | null) =>
    w === null || w.resets_at === null ? null : Math.max(0, (w.resets_at - now) / 3_600_000)
  const session = sessionOf(a)
  const weekRoom = Math.max(0, 100 - (a.week?.percent ?? 0))
  const sessionRoom = session === 'running' ? Math.max(0, 100 - (a.session?.percent ?? 0)) : 100
  const room = Math.min(weekRoom, sessionRoom)
  const untilWeek = Math.min(WEEK_H, hours(a.week) ?? WEEK_H)
  const untilSession = hours(a.session)
  let score = 100 * (1 - untilWeek / WEEK_H)
  if (session === 'running' && untilSession !== null && untilSession < 5) {
    score += 20 * (room / 100) * (1 - untilSession / 5)
  }
  const freesSoon =
    session === 'running' && weekRoom >= MIN_ROOM && untilSession !== null && untilSession <= 0.25
  const usable = a.status === 'ok' && a.usage_at !== null && (room >= MIN_ROOM || freesSoon)
  return { account: a, score, room, usable }
}

/** The other accounts, best first: the usable by score, then room, then no session limit; the rest after. */
function rank(others: SubsAccount[], now: number): Rated[] {
  const unlimited = (r: Rated) => Number(sessionOf(r.account) === 'none')
  const loggedOut = (r: Rated) => Number(r.account.status !== 'ok')
  return others
    .map(a => rate(a, now))
    .sort(
      (x, y) =>
        Number(y.usable) - Number(x.usable) ||
        loggedOut(x) - loggedOut(y) ||
        y.score - x.score ||
        y.room - x.room ||
        unlimited(y) - unlimited(x),
    )
}

/** The order ‹ › was paging through, kept while the person browses; accounts new since go last. */
function keep(ids: string[], ranked: Rated[]): Rated[] {
  const byId = new Map(ranked.map(r => [r.account.id, r]))
  const kept = ids.flatMap(id => byId.get(id) ?? [])
  return [...kept, ...ranked.filter(r => !ids.includes(r.account.id))]
}

// ---- How it reads -----------------------------------------------------------------------------

// Widths in terminal cells. A Button is `[ label ]` on the terminal; a plain one, its label alone.
const SWITCH = '[ Switch ]'.length
/** Between the account on the right and the pager. */
const GAP = 2
/** At least, between the live account and the other one. */
const MIDDLE = 3
/** What the prompt's own padding takes from the screen's width. */
const MARGIN = 4
/** ` ‹ 2/5 › `: two padded arrows and the counter. */
const pagerWidth = (n: number) => 3 + cells(`${n}/${n}`) + 3

/** Cells a text takes: one per character, which holds for the names, numbers and times drawn here. */
const cells = (s: string) => [...s].length
const tone = (w: SubsWindow) => (w.percent >= 85 ? 'error' : w.percent >= 60 ? 'warning' : undefined)

/** A piece of a line, measured and drawn alike. */
type Run = { text: string; color?: 'error' | 'warning' | 'success'; dim?: boolean; bold?: boolean }
const width = (runs: Run[]) => runs.reduce((n, r) => n + cells(r.text), 0)

/** When a window resets, as short as it reads: `47m`, `2h05`, `18:30`, `tmrw 05:00`, `Sat 11:00`. */
function soon(w: SubsWindow): string | null {
  if (w.resets_short) {
    return w.resets_short
  }
  // An older claude-subs words it longer: "in 2h 05m", "in 47m", "today 18:30", "tomorrow 05:00".
  return (
    w.resets
      ?.replace(/^in (\d+)h (\d+)m$/, '$1h$2')
      .replace(/^in /, '')
      .replace(/^today /, '')
      .replace(/^tomorrow /, 'tmrw ') ?? null
  )
}

/** One window: `65% 1h05`, `full 40m`, `idle` (not started), or `none` where there is no window. */
function windowRuns(w: SubsWindow | null, times: boolean, none: string): Run[] {
  if (w === null) {
    return [{ text: none, dim: true }]
  }
  if (w.resets_at === null && w.percent === 0) {
    return [{ text: 'idle', dim: true }]
  }
  const level: Run =
    w.percent >= 100 ? { text: 'full', color: 'error' } : { text: `${Math.round(w.percent)}%`, color: tone(w) }
  const at = times ? soon(w) : null
  return at ? [level, { text: ` ${at}`, dim: true }] : [level]
}

/** `side 65% 1h05 │ 85% tmrw 05:00`: the 5-hour session (∞ where the plan has none), then the week. */
function usageRuns(a: SubsAccount, times: boolean): Run[] {
  if (a.status === 'needs_login') {
    return [{ text: `${a.short} needs /login`, dim: true }]
  }
  return [
    { text: a.short, bold: true },
    { text: ' ' },
    ...windowRuns(a.session, times, a.usage_at === null ? '–' : '∞'),
    { text: ' │ ', dim: true },
    ...windowRuns(a.week, times, '–'),
  ]
}

const LIVE: Run = { text: '● ', color: 'success' }

/**
 * The fullest band that fits one row: every reset time, then the other account's left out, then
 * the live one's too; narrower still, the other account goes on a row of its own. Measured on the
 * widest of the others, so paging never changes the form.
 */
function fit(room: number, live: SubsAccount | undefined, others: SubsAccount[]) {
  const left = (times: boolean) => (live ? width([LIVE, ...usageRuns(live, times)]) : cells('No live account'))
  const pager = others.length > 1 ? GAP + pagerWidth(others.length) : 0
  const right = (times: boolean) =>
    others.length === 0
      ? 0
      : Math.max(...others.map(a => width(usageRuns(a, times)) + (a.status === 'ok' ? 1 + SWITCH : 0))) + pager
  for (const [liveTimes, otherTimes] of [
    [true, true],
    [true, false],
    [false, false],
  ] as const) {
    if (left(liveTimes) + MIDDLE + right(otherTimes) <= room) {
      return { liveTimes, otherTimes, ownRow: false }
    }
  }
  return { liveTimes: left(true) <= room, otherTimes: right(true) <= room, ownRow: true }
}

/**
 * The band: on the left the live account; on the right one other account, the best to switch to
 * unless ‹ › moved it in the last 30 seconds, with its Switch button (the accent color when it is
 * a better pick than the live one) and the pager. Null with no accounts.
 */
async function band($: Engine, ui: ElementTable, columns: number) {
  const st = await read($, status)
  if (st === null || st.accounts.length === 0) {
    return null
  }
  const switching = await read($, busy)
  const { Box, Button, Text } = ui
  const now = await $.clock.now()
  const live = st.accounts.find(a => a.active)
  const others = st.accounts.filter(a => !a.active)
  const f = fit(columns - MARGIN, live, others)

  const ranked = rank(others, now)
  const was = await read($, browse)
  const browsing = was !== null && now - was.at < SNAP_MS
  const order = browsing ? keep(was.order, ranked) : ranked
  const at = browsing ? Math.min(was.page, order.length - 1) : 0
  const pick = order[at]
  const liveRated = live ? rate(live, now) : null
  const better = pick?.usable === true && (liveRated === null || !liveRated.usable || pick.score > liveRated.score)
  const turn = (step: number) => async () => {
    const pressed = await $.clock.now()
    await update($, browse, () => ({
      order: order.map(r => r.account.id),
      page: (at + step + order.length) % order.length,
      at: pressed,
    }))
  }

  const draw = (runs: Run[]) => (
    <Text>
      {runs.map(r => (
        <Text color={r.color} dimColor={r.dim} weight={r.bold ? 'bold' : undefined}>
          {r.text}
        </Text>
      ))}
    </Text>
  )

  const left = live ? draw([LIVE, ...usageRuns(live, f.liveTimes)]) : <Text dimColor>No live account</Text>

  const right =
    switching !== null ? (
      <Text dimColor>switching to {switching}…</Text>
    ) : pick === undefined ? null : (
      <Box flexDirection="row" columnGap={GAP}>
        <Box flexDirection="row" columnGap={1}>
          {draw(usageRuns(pick.account, f.otherTimes))}
          {pick.account.status === 'ok' && (
            <Button
              key={`switch-${pick.account.id}`}
              label={`Switch to ${pick.account.short}`}
              variant={better ? 'primary' : undefined}
              onPress={() => switchTo($, pick.account)}
            >
              Switch
            </Button>
          )}
        </Box>
        {order.length > 1 && (
          <Box flexDirection="row">
            <Button key="page-prev" label="Previous account" plain dimColor onPress={turn(-1)}>
              {' ‹ '}
            </Button>
            <Text dimColor>{`${at + 1}/${order.length}`.padStart(cells(`${order.length}/${order.length}`))}</Text>
            <Button key="page-next" label="Next account" plain dimColor onPress={turn(1)}>
              {' › '}
            </Button>
          </Box>
        )}
      </Box>
    )

  return f.ownRow ? (
    <Box flexDirection="column">
      {left}
      {right}
    </Box>
  ) : (
    <Box flexDirection="row" justifyContent="space-between" columnGap={MIDDLE} width="100%">
      {left}
      {right}
    </Box>
  )
}

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    where = await locate($)
    const isHidden = (await $.store.get('hidden')) === true
    await update($, hidden, () => isHidden)
    const isAbove = (await $.store.get('placement')) === 'above'
    await update($, placement, () => (isAbove ? 'above' : 'below'))
    await $.command.register({
      name: 'subs',
      description: 'Claude Subs: your accounts and their usage; /subs <account> switches to one',
      argumentHint: '[account | refresh | hide | show | above | below]',
    })
    await load($, true)
    if (e.isInteractive) {
      $.clock.every(5_000, () => {
        void watch($)
      })
    }
    return started
  })

  // Under the prompt: the engine's hint line (`? for shortcuts`, `esc to interrupt`), then the band.
  on('ui.render', { component: 'PromptHint' }, async ($, e, next) => {
    if ((await read($, hidden)) || (await read($, placement)) !== 'below') {
      return next(e)
    }
    const row = await band($, $.ui.resolve(e), e.viewport?.columns ?? 120)
    if (row === null) {
      return next(e)
    }
    const { Box } = $.ui.resolve(e)
    return (
      <Box flexDirection="column">
        {await next(e)}
        {row}
      </Box>
    )
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    if (e.props.hasSurvey || (await read($, hidden)) || (await read($, placement)) !== 'above') {
      return next(e)
    }
    return (await band($, $.ui.resolve(e), e.props.bodyColumns)) ?? next(e)
  })

  on('command.run', { command: 'subs' }, async ($, e) => {
    const arg = e.args.trim()
    if (arg === 'hide' || arg === 'show') {
      await $.store.set('hidden', arg === 'hide')
      await update($, hidden, () => arg === 'hide')
      return { text: arg === 'hide' ? 'Claude Subs band hidden; /subs show brings it back.' : 'Claude Subs band shown.' }
    }
    if (arg === 'above' || arg === 'below') {
      await $.store.set('placement', arg)
      await update($, placement, () => arg)
      await update($, hidden, () => false)
      await $.store.set('hidden', false)
      return { text: `Claude Subs band ${arg} the prompt.` }
    }
    if (arg === 'refresh') {
      const r = await subs($, ['refresh'], 120_000)
      await load($, true)
      return { text: r.ok ? r.out : `claude-subs: ${r.err}` }
    }
    if (arg) {
      const r = await subs($, ['switch', arg], 120_000)
      await load($, true)
      if (r.ok) {
        void tick($)
      }
      return { text: r.ok ? r.out : `Switch failed: ${r.err}` }
    }
    const r = await subs($, ['status'])
    await load($, true)
    if (!r.ok) {
      return { text: `claude-subs did not answer: ${r.err}` }
    }
    return {
      text: `${r.out}\n\n/subs <account> switches (its name, email or number), as the band's buttons do; /subs above or /subs below moves the band.`,
    }
  })

  // The outgoing login's latest tokens are saved before /login replaces them.
  on('command.run', { command: 'login' }, async ($, e, next) => {
    await subs($, ['sync'], 15_000)
    return next(e)
  }).catch(($, e, next) => next(e))

  // /logout revokes the login at Anthropic, the saved copy with it: asked twice.
  on('command.run', { command: 'logout' }, async ($, e, next) => {
    const now = await $.clock.now()
    if (logoutAskedAt !== null && now - logoutAskedAt < 30_000) {
      logoutAskedAt = null
      return next(e)
    }
    logoutAskedAt = now
    return {
      text:
        'Claude Subs: /logout revokes this login at Anthropic, and the copy Claude Subs saved goes with it. ' +
        'To use another account, switch with the band under the prompt or /subs <account>. ' +
        'Run /logout again within 30 seconds to log out anyway.',
    }
  }).catch(($, e, next) => next(e))
}
