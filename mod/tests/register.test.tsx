import { describe, expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

const NOW = Date.UTC(2026, 10, 7, 15, 0)
const HOUR = 3_600_000

const HINT = {
  plugin: 'subs-switcher',
  component: 'PromptHint',
  props: { isDraft: false, isWorking: false, hint: '? for shortcuts' },
} as const

const ABOVE = {
  plugin: 'subs-switcher',
  component: 'AbovePrompt',
  props: {
    hasSurvey: false,
    isWorking: false,
    maxRows: 10,
    bodyColumns: 120,
    scroll: { offset: 0, bodyRows: 9 },
    view: {},
  },
} as const

/** A window `hours` from NOW, worded short as claude-subs words it; no hours, not started. */
const window = (label: string, percent: number, hours: number | null, short: string | null) => ({
  label,
  percent,
  resets_at: hours === null ? null : NOW + hours * HOUR,
  resets: short,
  resets_exact: short,
  resets_short: short,
})
const idle = window('Session', 0, null, null)

type Win = ReturnType<typeof window>

const account = (id: string, short: string, session: Win | null, week: Win, status = 'ok', active = false) => ({
  id,
  email: `${short}@example.com`,
  short,
  name: null,
  org: 'Org',
  plan: 'Team · Max 5x',
  active,
  status,
  error: null,
  session,
  week,
  scoped: [],
  usage_at: NOW - 60_000,
  usage_source: 'api',
  login_expires_at: null,
})

/**
 * The live work, and five others. Best first: lab (its week resets soonest, with room), oss (no
 * 5-hour limit), side; then spare, under 10% of room though it resets sooner; client needs a login.
 */
const accounts = (liveWeek = 80) => [
  account('A', 'work', window('Session', 42, 2.08, '2h05'), window('Week', liveWeek, 20, 'tmrw 11:00'), 'ok', true),
  account('B', 'side', idle, window('Week', 30, 100, 'Wed 19:00')),
  account('C', 'client', idle, window('Week', 0, null, null), 'needs_login'),
  account('D', 'lab', idle, window('Week', 70, 30, 'Sun 21:00')),
  account('E', 'oss', null, window('Week', 50, 60, 'Tue 03:00')),
  account('F', 'spare', idle, window('Week', 95, 10, '01:00')),
]

const status = (liveWeek?: number) => ({
  format: 1,
  app_version: '0.3.0',
  driver: 'command',
  exe: '/opt/claude-subs/bin/claude-subs',
  active: 'A',
  notice: null,
  at: NOW,
  accounts: accounts(liveWeek),
})

const LOGOUT = {
  command: 'logout',
  args: '',
  origin: { kind: 'composer' },
  presentation: { isFullscreen: true, columns: 120 },
} as const

const command = (args: string) =>
  ({
    command: 'subs',
    args,
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: 120 },
  }) as const

/** The hint line's site on a screen `columns` wide. */
const hint = (columns: number) => ({ ...HINT, surface: 'terminal', viewport: { columns, rows: 40 } }) as const

/** The world beneath the mod: a status file, a binary that records its runs, the engine's drawings. */
function world(on: On, ran: string[][], st = status()) {
  const clock = mock.clock(on, { now: NOW })
  mock.env(on, { HOME: '/home/u' })
  mock.store(on, {})
  on('session.start', (_, e) => ({ cwd: e.cwd }))
  on('command.register', (_, e) => ({ value: { command: e.name } }))
  // The engine hands the hook the path made absolute for the host (a drive letter on Windows).
  on('fs.stat', (_, e) => {
    if (e.path.replaceAll('\\', '/').endsWith('/home/u/.local/share/claude-subs/status.json')) {
      return { value: { kind: 'file', size: 1, mtimeMs: 1, isLink: false } }
    }
    return { deny: `no ${e.path}` }
  })
  on('fs.read', () => ({ value: JSON.stringify(st) }))
  on('process.run', (_, e) => {
    ran.push([...e.argv])
    return {
      value: {
        exitCode: 0,
        stdout: 'Switched to lab@example.com.',
        stderr: '',
        isStdoutTruncated: false,
        isStderrTruncated: false,
      },
    }
  })
  on('ui.toast', () => ({ value: undefined }))
  // What the engine draws at each site when the mod passes.
  on('ui.render', ($, e) => {
    const { Text } = $.ui.resolve(e)
    return <Text>{e.component === 'PromptHint' ? '? for shortcuts' : 'engine band'}</Text>
  })
  return clock
}

describe('register', () => {
  test('the live account on the left, the best other on the right, and a Switch that switches', async ($, on) => {
    const ran: string[][] = []
    world(on, ran)
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })

    const ui = await $.ui.mount(hint(200))
    expect(await ui.find({ type: 'Text', text: /for shortcuts/ })).toBeDefined()
    expect((await ui.find({ type: 'Text', text: /work/ }))?.text).toBe('● work 42% 2h05 │ 80% tmrw 11:00')
    expect((await ui.find({ type: 'Text', text: /lab/ }))?.text).toBe('lab idle │ 70% Sun 21:00')
    expect(await ui.find({ type: 'Text', text: /^1\/5$/ })).toBeDefined()
    // work still has room and its week resets first: lab is no better yet.
    expect((await ui.find({ key: 'switch-D' }))?.props.variant).toBeUndefined()

    await ui.press({ key: 'switch-D' })
    expect(ran[0]).toEqual(['/opt/claude-subs/bin/claude-subs', 'switch', 'D'])
    await ui.unmount()

    const above = await $.ui.mount({ ...ABOVE, surface: 'terminal' })
    expect((await above.find({ type: 'Text' }))?.text).toBe('engine band')
    await above.unmount()
  })

  test('‹ › page through the others best first, and the band goes back to the best after 30 s', async ($, on) => {
    const clock = world(on, [])
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: true })
    const ui = await $.ui.mount(hint(200))
    // The other account's whole line (its name alone is a Text too, as is the live one's).
    const shown = async () => (await ui.find({ type: 'Text', text: /^dev\d+ \S/ }))?.text

    await ui.press({ key: 'page-next' })
    expect(await shown()).toBe('oss ∞ │ 50% Tue 03:00')
    expect(await ui.find({ type: 'Text', text: /^2\/5$/ })).toBeDefined()
    await ui.press({ key: 'page-next' })
    expect(await shown()).toBe('side idle │ 30% Wed 19:00')
    await ui.press({ key: 'page-next' })
    expect(await shown()).toBe('spare idle │ 95% 01:00')
    await ui.press({ key: 'page-next' })
    expect(await shown()).toBe('client needs /login')
    expect(await ui.find({ key: 'switch-C' })).toBeUndefined()
    await ui.press({ key: 'page-next' })
    expect(await shown()).toMatch(/^lab/)
    await ui.press({ key: 'page-prev' })
    expect(await shown()).toMatch(/^client/)

    await clock.advance(20_000)
    expect(await shown()).toMatch(/^client/)
    await clock.advance(15_000)
    expect(await shown()).toMatch(/^lab/)
    expect(await ui.find({ type: 'Text', text: /^1\/5$/ })).toBeDefined()
    await ui.unmount()
  })

  test('Switch is the accent button once the live account is out of room', async ($, on) => {
    world(on, [], status(95))
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })
    const ui = await $.ui.mount(hint(200))
    expect((await ui.find({ key: 'switch-D' }))?.props.variant).toBe('primary')
    await ui.unmount()
  })

  test('narrower, the other account drops its times first, then the live one', async ($, on) => {
    world(on, [])
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })

    let ui = await $.ui.mount(hint(80))
    expect((await ui.find({ type: 'Text', text: /work/ }))?.text).toBe('● work 42% 2h05 │ 80% tmrw 11:00')
    expect((await ui.find({ type: 'Text', text: /lab/ }))?.text).toBe('lab idle │ 70%')
    await ui.unmount()

    ui = await $.ui.mount(hint(60))
    expect((await ui.find({ type: 'Text', text: /work/ }))?.text).toBe('● work 42% │ 80%')
    await ui.unmount()
  })

  test('/subs above moves the band over the prompt', async ($, on) => {
    world(on, [])
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })
    expect((await $.command.run(command('above'))).text).toContain('above the prompt')

    const above = await $.ui.mount({ ...ABOVE, surface: 'terminal' })
    expect(await above.find({ key: 'switch-D' })).toBeDefined()
    await above.unmount()
    const under = await $.ui.mount(hint(200))
    expect(await under.find({ type: 'Text', text: /work/ })).toBeUndefined()
    expect((await under.find({ type: 'Text' }))?.text).toBe('? for shortcuts')
    await under.unmount()
  })

  test('/logout is asked twice', async ($, on) => {
    let loggedOut = false
    mock.clock(on)
    mock.env(on, { HOME: '/home/u' })
    mock.store(on, {})
    on('session.start', (_, e) => ({ cwd: e.cwd }))
    on('command.register', (_, e) => ({ value: { command: e.name } }))
    on('fs.stat', (_, e) => ({ deny: `no ${e.path}` }))
    on('command.run', { command: 'logout' }, () => {
      loggedOut = true
      return { text: 'Logged out' }
    })

    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })
    const first = await $.command.run(LOGOUT)
    expect(first.text).toContain('revokes this login')
    expect(loggedOut).toBe(false)
    await $.command.run(LOGOUT)
    expect(loggedOut).toBe(true)
  })
})
