// Claude Subs in Claude Code: a band above the prompt with the live account's usage and a button
// per saved account; a press switches the login, and every running session follows (the same way
// a /login in one session reaches the others).
//
// The accounts come from `status.json`, which the claude-subs binary writes (the Windows tray app,
// or each command this mod runs). Switching runs `claude-subs switch <id>`. Where no tray app
// keeps the readings fresh (Linux), the mod runs `claude-subs tick` itself: once the status is a
// minute and a half old, and when Claude Code's credentials file changes (a token refresh or a
// /login), so the latest tokens are always saved before anything can replace them.

import { atom, read, update } from 'claude-code'
import type { EngineInterface, Register } from 'claude-code'

import type { SubsAccount, SubsStatus, SubsWindow } from '../types'

type Engine = EngineInterface

const status = atom({ plugin: 'subs-switcher', key: 'status' } as const, null, { shape: 'status-1' })
const busy = atom({ plugin: 'subs-switcher', key: 'busy' } as const, null)
const hidden = atom({ plugin: 'subs-switcher', key: 'hidden' } as const, false)

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
    // The new account's first reading.
    void tick($)
  } else {
    $.ui.toast(`Switch failed: ${r.err.split('\n')[0] || 'claude-subs did not answer'}`, { timeoutMs: 8_000 })
  }
}

const tone = (w: SubsWindow | null) =>
  w === null ? undefined : w.percent >= 85 ? 'error' : w.percent >= 60 ? 'warning' : undefined
const pct = (w: SubsWindow | null) => (w === null ? '–' : `${Math.round(w.percent)}`)

export const register: Register = on => {
  on('session.start', async ($, e, next) => {
    const started = await next(e)
    where = await locate($)
    const isHidden = (await $.store.get('hidden')) === true
    await update($, hidden, () => isHidden)
    await $.command.register({
      name: 'subs',
      description: 'Claude Subs: your accounts and their usage; /subs <account> switches to one',
      argumentHint: '[account | refresh | hide | show]',
    })
    await load($, true)
    if (e.isInteractive) {
      $.clock.every(5_000, () => {
        void watch($)
      })
    }
    return started
  })

  on('ui.render', { component: 'AbovePrompt' }, async ($, e, next) => {
    if (e.props.hasSurvey || (await read($, hidden))) {
      return next(e)
    }
    const st = await read($, status)
    if (st === null || st.accounts.length === 0) {
      return next(e)
    }
    const switching = await read($, busy)
    const { Box, Button, Text } = $.ui.resolve(e)
    const live = st.accounts.find(a => a.active)
    const others = st.accounts.filter(a => !a.active)
    const roomy = e.props.bodyColumns >= 90
    const resets = (w: SubsWindow | null) =>
      roomy && w?.resets ? <Text dimColor> {w.resets}</Text> : null

    return (
      <Box flexDirection="row" flexWrap="wrap" columnGap={2}>
        {live ? (
          <Text>
            <Text color="success">●</Text> <Text weight="bold">{live.short}</Text>
            {'  '}session <Text color={tone(live.session)}>{pct(live.session)}%</Text>
            {resets(live.session)}
            {'  '}week <Text color={tone(live.week)}>{pct(live.week)}%</Text>
            {resets(live.week)}
          </Text>
        ) : (
          <Text dimColor>No live account</Text>
        )}
        {others.map(a =>
          a.status === 'needs_login' ? (
            <Text dimColor>{a.short}: needs /login</Text>
          ) : (
            <Button
              key={`switch-${a.id}`}
              label={`Switch to ${a.short}`}
              dimColor={switching !== null}
              onPress={() => switchTo($, a)}
            >
              {a.short} <Text color={tone(a.session)}>{pct(a.session)}</Text>/
              <Text color={tone(a.week)}>{pct(a.week)}</Text>
            </Button>
          ),
        )}
        {switching !== null && <Text dimColor>switching to {switching}…</Text>}
      </Box>
    )
  })

  on('command.run', { command: 'subs' }, async ($, e) => {
    const arg = e.args.trim()
    if (arg === 'hide' || arg === 'show') {
      await $.store.set('hidden', arg === 'hide')
      await update($, hidden, () => arg === 'hide')
      return { text: arg === 'hide' ? 'Claude Subs band hidden; /subs show brings it back.' : 'Claude Subs band shown.' }
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
      text: `${r.out}\n\n/subs <account> switches (its name, email or number); the band above the prompt does too.`,
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
        'To use another account, switch with the band above the prompt or /subs <account>. ' +
        'Run /logout again within 30 seconds to log out anyway.',
    }
  }).catch(($, e, next) => next(e))
}
