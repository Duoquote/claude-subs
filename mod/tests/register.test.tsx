import { describe, expect, mock, test } from 'claude-code/testing'
import type { On } from 'claude-code'

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

const window = (label: string, percent: number, resets: string) => ({
  label,
  percent,
  resets_at: 0,
  resets,
  resets_exact: resets,
})

const account = (id: string, short: string, active: boolean, s: number, w: number, status = 'ok') => ({
  id,
  email: `${short}@example.com`,
  short,
  name: null,
  org: 'Org',
  plan: 'Team · Max 5x',
  active,
  status,
  error: null,
  session: window('Session', s, 'in 2h 05m'),
  week: window('Week', w, 'tomorrow 11:00'),
  scoped: [],
  usage_at: 0,
  usage_source: 'api',
  login_expires_at: null,
})

const LOGOUT = {
  command: 'logout',
  args: '',
  origin: { kind: 'composer' },
  presentation: { isFullscreen: true, columns: 120 },
} as const

const STATUS = {
  format: 1,
  app_version: '0.2.0',
  driver: 'command',
  exe: '/opt/claude-subs/bin/claude-subs',
  active: 'A',
  notice: null,
  at: 0,
  accounts: [
    account('A', 'work', true, 42, 91),
    account('B', 'side', false, 12, 30),
    account('C', 'client', false, 0, 0, 'needs_login'),
    account('D', 'lab', false, 70, 88),
  ],
}

const command = (args: string) =>
  ({
    command: 'subs',
    args,
    origin: { kind: 'composer' },
    presentation: { isFullscreen: true, columns: 120 },
  }) as const

/** The world beneath the mod: a status file, a binary that records its runs, the engine's drawings. */
function world(on: On, ran: string[][]) {
  mock.clock(on)
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
  on('fs.read', () => ({ value: JSON.stringify(STATUS) }))
  on('process.run', (_, e) => {
    ran.push([...e.argv])
    return {
      value: {
        exitCode: 0,
        stdout: 'Switched to side@example.com.',
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
}

/** The hint line's site on a screen `columns` wide. */
const hint = (columns: number) => ({ ...HINT, surface: 'terminal', viewport: { columns, rows: 40 } }) as const

describe('register', () => {
  test('a wide band keeps the hint line, shows every account with its resets, and switches on a press', async ($, on) => {
    const ran: string[][] = []
    world(on, ran)
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })

    const ui = await $.ui.mount(hint(300))
    expect(await ui.find({ type: 'Text', text: /for shortcuts/ })).toBeDefined()
    const live = await ui.find({ type: 'Text', text: /work/ })
    expect(live?.text).toContain('session 42% in 2h 05m')
    expect(live?.text).toContain('week 91% tomorrow 11:00')
    expect((await ui.find({ type: 'Text', text: /side/ }))?.text).toContain('week 30% tomorrow 11:00')
    expect(await ui.find({ type: 'Text', text: /client  needs \/login/ })).toBeDefined()
    expect(await ui.find({ key: 'switch-C' })).toBeUndefined()
    expect(await ui.find({ key: 'switch-D' })).toBeDefined()
    expect(await ui.find({ key: 'page-next' })).toBeUndefined()

    await ui.press({ key: 'switch-B' })
    expect(ran[0]).toEqual(['/opt/claude-subs/bin/claude-subs', 'switch', 'B'])
    await ui.unmount()

    const above = await $.ui.mount({ ...ABOVE, surface: 'terminal' })
    expect((await above.find({ type: 'Text' }))?.text).toBe('engine band')
    await above.unmount()
  })

  test('a narrower band pages through the other accounts', async ($, on) => {
    world(on, [])
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })

    const ui = await $.ui.mount(hint(200))
    expect((await ui.find({ type: 'Text', text: /side/ }))?.text).toContain('session 12% in 2h 05m')
    expect(await ui.find({ key: 'switch-B' })).toBeDefined()
    expect(await ui.find({ key: 'switch-D' })).toBeUndefined()
    expect(await ui.find({ type: 'Text', text: /^1\/3$/ })).toBeDefined()

    await ui.press({ key: 'page-next' })
    expect(await ui.find({ type: 'Text', text: /client  needs \/login/ })).toBeDefined()
    expect(await ui.find({ key: 'switch-B' })).toBeUndefined()
    await ui.press({ key: 'page-next' })
    expect(await ui.find({ key: 'switch-D' })).toBeDefined()
    expect(await ui.find({ type: 'Text', text: /^3\/3$/ })).toBeDefined()
    await ui.press({ key: 'page-next' })
    expect(await ui.find({ key: 'switch-B' })).toBeDefined()
    await ui.press({ key: 'page-prev' })
    expect(await ui.find({ key: 'switch-D' })).toBeDefined()
    await ui.unmount()
  })

  test('at 120 columns the live account keeps its resets and the others drop theirs', async ($, on) => {
    world(on, [])
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })

    const ui = await $.ui.mount(hint(120))
    expect((await ui.find({ type: 'Text', text: /work/ }))?.text).toContain('week 91% tomorrow 11:00')
    const other = await ui.find({ type: 'Text', text: /side/ })
    expect(other?.text).toContain('week 30%')
    expect(other?.text).not.toContain('tomorrow')
    expect(await ui.find({ type: 'Text', text: /^1\/3$/ })).toBeDefined()
    await ui.unmount()
  })

  test('/subs above moves the band over the prompt', async ($, on) => {
    world(on, [])
    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })
    expect((await $.command.run(command('above'))).text).toContain('above the prompt')

    const above = await $.ui.mount({ ...ABOVE, surface: 'terminal' })
    expect(await above.find({ key: 'switch-B' })).toBeDefined()
    await above.unmount()
    const hint = await $.ui.mount({ ...HINT, surface: 'terminal' })
    expect(await hint.find({ type: 'Text', text: /work/ })).toBeUndefined()
    expect((await hint.find({ type: 'Text' }))?.text).toBe('? for shortcuts')
    await hint.unmount()
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
