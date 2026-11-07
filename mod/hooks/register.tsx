// Claude Subs in Claude Code: a band under the prompt (or above it). On its left the live account's
// usage; on its right the other saved accounts, a page at a time, each with a Switch button. A press
// switches the login, and every running session follows (the same way a /login in one session
// reaches the others).
//
// The accounts come from `status.json`, which the claude-subs binary writes (the Windows tray app,
// or each command this mod runs). Switching runs `claude-subs switch <id>`. Where no tray app
// keeps the readings fresh (Linux), the mod runs `claude-subs tick` itself: once the status is a
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
/** Which page of the other accounts the band shows. */
const page = atom({ plugin: 'subs-switcher', key: 'page' } as const, 0)

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

/** Every few seconds: pick up a new status; where no tray app runs, keep the readings going. */
async function watch($: Engine) {
  if (!where) {
    return
  }
  const st = await load($)
  const now = await $.clock.now()
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
    // The others are now another list.
    await update($, page, () => 0)
    // The new account's first reading.
    void tick($)
  } else {
    $.ui.toast(`Switch failed: ${r.err.split('\n')[0] || 'claude-subs did not answer'}`, { timeoutMs: 8_000 })
  }
}

// The band's widths, in terminal cells. A Button is `[ label ]` on the terminal.
const SWITCH = '[ Switch ]'.length
const ARROW = '[ ‹ ]'.length
/** Between the accounts on the right. */
const GAP = 2
/** At least, between the live account and the others. */
const MIDDLE = 3
/** What the prompt's own padding takes from the screen's width. */
const MARGIN = 4

/** Cells a text takes: one per character, which holds for the names, numbers and times drawn here. */
const cells = (s: string) => [...s].length
const pct = (w: SubsWindow | null) => (w === null ? '–' : `${Math.round(w.percent)}%`)
const tone = (w: SubsWindow | null) =>
  w === null ? undefined : w.percent >= 85 ? 'error' : w.percent >= 60 ? 'warning' : undefined

// The band's texts as strings, to measure them; `band` draws the same.
const meterText = (label: string, w: SubsWindow | null, resets: boolean) =>
  `${label} ${pct(w)}${resets && w?.resets ? ` ${w.resets}` : ''}`
const usageText = (a: SubsAccount, resets: boolean) =>
  `${a.short}  ${meterText('session', a.session, resets)}  ${meterText('week', a.week, resets)}`
const liveWidth = (a: SubsAccount | undefined, resets: boolean) =>
  a ? cells(`● ${usageText(a, resets)}`) : cells('No live account')
const otherWidth = (a: SubsAccount, resets: boolean) =>
  a.status === 'needs_login' ? cells(`${a.short}  needs /login`) : cells(usageText(a, resets)) + 1 + SWITCH
const pagerWidth = (pages: number) => ARROW + 1 + cells(`${pages}/${pages}`) + 1 + ARROW

/** How the band is laid out: whose reset times show, how many other accounts a page holds. */
type Fit = { liveResets: boolean; otherResets: boolean; perPage: number; ownRow: boolean }

/** How many of `others` fit side by side in `room`: all of them, or as many as fit beside the pager. */
function perPage(room: number, others: SubsAccount[], resets: boolean): number {
  const widest = Math.max(...others.map(a => otherWidth(a, resets)))
  const fits = (r: number) => Math.floor((r + GAP) / (widest + GAP))
  if (fits(room) >= others.length) {
    return others.length
  }
  return Math.max(0, fits(room - GAP - pagerWidth(others.length)))
}

/**
 * The fullest band that fits one row: every reset time, then the others' left out, then the live
 * account's too. Narrower still, the others go on a row of their own, one or more a page.
 */
function fit(room: number, live: SubsAccount | undefined, others: SubsAccount[]): Fit {
  if (others.length === 0) {
    return { liveResets: liveWidth(live, true) <= room, otherResets: false, perPage: 1, ownRow: false }
  }
  for (const [liveResets, otherResets] of [
    [true, true],
    [true, false],
    [false, false],
  ] as const) {
    const n = perPage(room - liveWidth(live, liveResets) - MIDDLE, others, otherResets)
    if (n > 0) {
      return { liveResets, otherResets, perPage: n, ownRow: false }
    }
  }
  const otherResets = perPage(room, others, true) > 0
  return {
    liveResets: liveWidth(live, true) <= room,
    otherResets,
    perPage: Math.max(1, perPage(room, others, otherResets)),
    ownRow: true,
  }
}

/**
 * The band: on the left the live account's usage; on the right the other saved accounts, a page
 * at a time, each with its usage and a Switch button. Null with no accounts.
 */
async function band($: Engine, ui: ElementTable, columns: number) {
  const st = await read($, status)
  if (st === null || st.accounts.length === 0) {
    return null
  }
  const switching = await read($, busy)
  const { Box, Button, Text } = ui
  const live = st.accounts.find(a => a.active)
  const others = st.accounts.filter(a => !a.active)
  const f = fit(columns - MARGIN, live, others)
  const pages = Math.max(1, Math.ceil(others.length / f.perPage))
  const at = Math.min(await read($, page), pages - 1)
  const shown = others.slice(at * f.perPage, (at + 1) * f.perPage)
  const turn = (to: number) => () => update($, page, () => (to + pages) % pages)

  const meter = (label: string, w: SubsWindow | null, resets: boolean) => (
    <Text>
      {label} <Text color={tone(w)}>{pct(w)}</Text>
      {resets && w?.resets ? <Text dimColor> {w.resets}</Text> : null}
    </Text>
  )
  const usage = (a: SubsAccount, resets: boolean) => (
    <Text>
      {'  '}
      {meter('session', a.session, resets)}
      {'  '}
      {meter('week', a.week, resets)}
    </Text>
  )

  const left = live ? (
    <Text>
      <Text color="success">●</Text> <Text weight="bold">{live.short}</Text>
      {usage(live, f.liveResets)}
    </Text>
  ) : (
    <Text dimColor>No live account</Text>
  )

  const right =
    switching !== null ? (
      <Text dimColor>switching to {switching}…</Text>
    ) : others.length === 0 ? null : (
      <Box flexDirection="row" columnGap={GAP}>
        {shown.map(a =>
          a.status === 'needs_login' ? (
            <Text dimColor>{a.short}  needs /login</Text>
          ) : (
            <Box flexDirection="row" columnGap={1}>
              <Text>
                {a.short}
                {usage(a, f.otherResets)}
              </Text>
              <Button key={`switch-${a.id}`} label={`Switch to ${a.short}`} onPress={() => switchTo($, a)}>
                Switch
              </Button>
            </Box>
          ),
        )}
        {pages > 1 && (
          <Box flexDirection="row" columnGap={1}>
            <Button key="page-prev" label="Previous accounts" onPress={turn(at - 1)}>
              ‹
            </Button>
            <Text dimColor>{`${at + 1}/${pages}`.padStart(cells(`${pages}/${pages}`))}</Text>
            <Button key="page-next" label="Next accounts" onPress={turn(at + 1)}>
              ›
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
