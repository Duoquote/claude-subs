# subs-switcher

The Claude Code half of [claude-subs](https://github.com/Duoquote/claude-subs): your Claude
accounts in a band above the prompt.

- **The live account:** session and weekly usage, with reset times.
- **A button per saved account:** click it (fullscreen mode) or focus the band with `ctrl+x tab`
  and press Enter. Claude Code switches to that account, and every running session follows from
  its next request.
- **`/subs`:** lists the accounts. `/subs <account>` switches, `/subs refresh` reads usage now, and
  `/subs hide` / `/subs show` toggle the band.
- **`/logout`** is asked twice. A logout revokes the login at Anthropic, which kills the saved copy
  too.

The mod does none of the work itself. It reads `status.json` in claude-subs' data folder and runs
the `claude-subs` binary (`switch`, `tick`, `sync`). On Linux, where no tray app runs, it also runs
`claude-subs tick` once the status is 90 seconds old, and whenever Claude Code's credentials file
changes. That way the latest tokens are always saved before a `/login` can replace them.

It is a mod (a plugin of function hooks), an early-access Claude Code feature whose API may
change between releases. `claude-subs setup` loads it by naming this folder in
`CLAUDE_CODE_PLUGIN_DIRS` in `~/.claude/settings.json`.
