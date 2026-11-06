import { describe, expect, mock, test } from 'claude-code/testing'

const BAND = {
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
  ],
}

describe('register', () => {
  test('the band shows the live account and switches on a press', async ($, on) => {
    const ran: string[][] = []
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
    // The engine's own band, drawn when the mod passes.
    on('ui.render', ($, e) => {
      const { Text } = $.ui.resolve(e)
      return <Text>engine</Text>
    })

    await $.session.start({ cwd: '/work', surface: 'terminal', isInteractive: false })
    const ui = await $.ui.mount({ ...BAND, surface: 'terminal' })

    expect(await ui.find({ type: 'Text', text: /engine/ })).toBeUndefined()
    const live = await ui.find({ type: 'Text', text: /work/ })
    expect(live?.text).toContain('session 42%')
    expect(live?.text).toContain('week 91%')
    expect(await ui.find({ type: 'Text', text: /client: needs \/login/ })).toBeDefined()
    expect(await ui.find({ key: 'switch-C' })).toBeUndefined()

    await ui.press({ key: 'switch-B' })
    expect(ran[0]).toEqual(['/opt/claude-subs/bin/claude-subs', 'switch', 'B'])
    await ui.unmount()
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
