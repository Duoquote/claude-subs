# Claude Subs

Switches Claude Code's login between your Claude accounts, and shows how much of each account's
session (5-hour) and weekly limit is used.

- **In Claude Code** (Windows and Linux): a band under the prompt shows the live account's usage,
  with a button for every other saved account. Click one and every running Claude Code session
  carries on with that account.
- **On Windows:** a tray app with the usage in the icon and a popover listing all accounts.
- **Anywhere:** a command line, `claude-subs status` / `switch <account>`.

## Install on Linux

```sh
curl -fsSL https://raw.githubusercontent.com/Duoquote/claude-subs/main/install.sh | sh
```

What the installer does:

1. Downloads the static binary for x86_64 or aarch64 from the latest release and checks its
   SHA-256.
2. Installs it in `~/.local/share/claude-subs/bin`, with a link in `~/.local/bin`.
3. Installs the Claude Code mod in `~/.local/share/claude-subs/mod`.
4. Runs `claude-subs setup`, which names the mod's folder in `CLAUDE_CODE_PLUGIN_DIRS` in
   `~/.claude/settings.json`. A backup goes to the data folder first.

Run it again to update. To remove, add `-s -- --uninstall` (`... | sh -s -- --uninstall`). The
saved logins stay until you delete `~/.local/share/claude-subs`.

## Install on Windows

Build it (below), start `target\release\claude-subs.exe`, then load the mod:

```powershell
target\release\claude-subs.exe setup --mod-dir "$PWD\mod"
```

## Use

- **Add an account:** run `/login` in any Claude Code session. The new login is saved on its own;
  there is no separate add step.
- **Switch:**
  - click a button in the band (fullscreen mode, `"tui": "fullscreen"`). With the band above the
    prompt (`/subs above`) you can also focus it with `ctrl+x tab` and press Enter;
  - type `/subs <account>`;
  - run `claude-subs switch <account>`;
  - on Windows, use the tray popover or menu.

  An account is named by its email, the part before the `@`, or its number in `claude-subs status`.
  Running sessions use the new account from their next request. This is the same mechanism that
  makes a `/login` in one session apply to the others.
- **Don't `/logout`.** Logout revokes the login at Anthropic, which also kills the saved copy. The
  mod asks before letting a `/logout` through.
- An account marked **needs /login** has a saved login that was refused: it expired after about 30
  days unused, or was revoked. Run `/login` with that account again.

### In Claude Code

| | |
|---|---|
| Band | Under the prompt, below the hint line: the live account's session and week, then `[ side 12/30 ]` (session/week %) per account. |
| `/subs` | Lists the accounts. |
| `/subs <account>` | Switches. |
| `/subs refresh` | Reads every account's usage now. |
| `/subs hide`, `/subs show` | Toggles the band. |
| `/subs above`, `/subs below` | Moves the band over the prompt or back under it. |

The mod is an early-access Claude Code feature (function hooks), and its API may change between
Claude Code releases. If a release breaks it, the command line keeps working.

### Command line

```text
claude-subs status [--json]     the saved accounts and their usage
claude-subs switch <account>    make it the live login
claude-subs refresh             read every account's usage now
claude-subs forget <account>    remove a saved account (not the live one)
claude-subs setup [--remove]    load (or stop loading) the Claude Code mod
```

### Windows tray

- **Tray icon:** the number is the fuller of session and week for the live account. The two bars
  below it are session (top) and week (bottom). Green under 60 %, amber from 60 %, red from 85 %.
  A red `!` means a limit is reached; `--` means no reading yet.
- **Hover:** the live account with its reset times, then the others as `name session/week`.
- **Left click:** the popover lists every saved account with its meters and a Switch button.
- **Right click:** switch from the menu, Refresh usage, Start at login, Quit.

**Reset times** follow the Windows 12/24-hour setting (24-hour elsewhere):

- Under 6 hours away, a countdown: `in 2h 05m`.
- Later: `today 18:30`, `tomorrow 11:00`, `Sat 11:00`.

## How it works

Claude Code (2.1.x) keeps the login in two files:

| File | Key | What |
|---|---|---|
| `~/.claude/.credentials.json` | `claudeAiOauth` | access and refresh tokens |
| `~/.claude.json` | `oauthAccount` | who is logged in (email, org, ...) |

Every Claude Code process re-checks `.credentials.json` before its requests and adopts a changed
file (the "sibling login"). Claude Code refreshes tokens under a lock directory,
`~/.claude/.oauth_refresh.lock` (plus the legacy `~/.claude.lock`). Each refresh rotates the
refresh token, so a token has to live in exactly one place at a time.

Claude Subs follows that rule:

- **Capture:** new tokens in `.credentials.json` come from a refresh by Claude Code or from a
  `/login`. Claude Subs asks the profile endpoint whose they are and saves them under that account.
  - On Windows, the tray app watches the file.
  - On Linux, the mod runs `claude-subs tick` when the file changes, and `claude-subs sync` before
    a `/login`.

  So the live account's latest tokens are saved before anything can overwrite them.
- **Switch:**
  1. Take Claude Code's refresh locks, so no refresh is in flight.
  2. Save the outgoing tokens.
  3. Back up `~/.claude.json` (the last 10 are kept).
  4. Write the incoming `oauthAccount` into `~/.claude.json` under its own `.lock`, keeping every
     other key.
  5. Atomically replace `claudeAiOauth` in `.credentials.json`. Other entries, such as MCP logins,
     are kept.
  6. Release the locks.
- **Usage:**
  - For the live account, Claude Subs uses Claude Code's own cached reading
    (`cachedUsageUtilization`) when it is newer. Otherwise it reads `GET /api/oauth/usage` itself
    with the live access token, at most every 5 minutes. It never refreshes the live tokens;
    Claude Code owns them.
  - The other accounts are read every 15 minutes with their saved tokens, which are refreshed when
    expired. That also keeps their 30-day refresh tokens alive.
  - On HTTP 429 it backs off, from 5 minutes up to an hour.
- **Several processes:** the tray app and each command change the vault only while holding
  `vault.lock`, and read it again first if another process saved in the meantime. Each of them also
  writes `status.json` (accounts and usage, no tokens), which is what the mod reads.

The usage, profile and token endpoints are the ones Claude Code itself calls. They are not a
documented public API and may change.

## Files

The data folder is `%LOCALAPPDATA%\claude-subs` on Windows and `~/.local/share/claude-subs` on
Linux (`$XDG_DATA_HOME/claude-subs`). Override it with `CLAUDE_SUBS_DATA_DIR`.

- `vault.dat`: the saved logins and last readings.
  - Windows: sealed with DPAPI, so only this Windows user on this machine can open it.
  - Linux: plain JSON readable by your user alone (0600), the way Claude Code keeps
    `.credentials.json` there.
- `status.json`: what the mod reads.
- `settings.json`: `poll_active_secs` (300), `poll_idle_secs` (900), `refresh_idle` (true).
- `backups/`: `~/.claude.json` before each switch, and `~/.claude/settings.json` before `setup`
  changes it.
- `logs/`: daily logs. Tokens are never logged. Set `CLAUDE_SUBS_LOG=debug` for more detail.

`CLAUDE_CONFIG_DIR` is honoured the same way Claude Code honours it.

## Build

```sh
cargo test                                 # the switch runs against a sandbox; no network
cargo build --release                      # Linux: the command line alone
# Windows, the tray app: the UI is embedded from ui/dist
cd ui && bun install && bun run build && cd .. && cargo build --release
claude plugin test mod                     # the mod's tests
```

Linux releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed.

The app icon comes from `app/icons/make-icon.ps1`. The tray icon is drawn in code
(`app/src/icon.rs`), at the notification area's pixel size so it stays sharp.
