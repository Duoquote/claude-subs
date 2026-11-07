# subs-switcher

The Claude Code half of [claude-subs](https://github.com/Duoquote/claude-subs): your Claude
accounts in a band under the prompt (or above it, with `/subs above`).

- **On the left, the live account:** `● work ∞ │ 79% tmrw 05:00`, its 5-hour session (`∞` where
  the plan has none, `idle` when not started) and its week, each with when it resets.
- **On the right, the best account to switch to:** the one whose weekly limit resets soonest with
  at least 10% of room, so no weekly room goes to waste. `‹ 1/5 ›` pages through the rest, best
  first, and the band goes back to the best 30 seconds later. Click **Switch** (fullscreen mode):
  Claude Code switches to that account, and every running session follows from its next request.
  Switch is drawn in the accent color when that account is a better pick than the live one. Above
  the prompt the band can also be focused with `ctrl+x tab`, then Enter presses a button.
- **`/subs`:** lists the accounts. `/subs <account>` switches, `/subs refresh` reads usage now, and
  `/subs hide` / `/subs show` toggle the band, `/subs above` / `/subs below` move it.
- **`/logout`** is asked twice. A logout revokes the login at Anthropic, which kills the saved copy
  too.

The mod does none of the work itself. It reads `status.json` in claude-subs' data folder and runs
the `claude-subs` binary (`switch`, `tick`, `sync`). On Linux, where no tray app runs, it also runs
`claude-subs tick` once the status is 90 seconds old, and whenever Claude Code's credentials file
changes. That way the latest tokens are always saved before a `/login` can replace them.

It is a mod (a plugin of function hooks), an early-access Claude Code feature whose API may
change between releases. `claude-subs setup` loads it by naming this folder in
`CLAUDE_CODE_PLUGIN_DIRS` in `~/.claude/settings.json`.
