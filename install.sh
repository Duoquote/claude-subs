#!/bin/sh
# claude-subs for Linux: the command line, and the Claude Code mod that puts your accounts above
# the prompt with a button per account.
#
#   curl -fsSL https://raw.githubusercontent.com/Duoquote/claude-subs/main/install.sh | sh
#
# Run it again to update. To remove (saved logins are kept unless you delete the data folder):
#
#   curl -fsSL https://raw.githubusercontent.com/Duoquote/claude-subs/main/install.sh | sh -s -- --uninstall
#
# CLAUDE_SUBS_VERSION=v0.2.0 picks a release (default: the latest).
set -eu

repo="Duoquote/claude-subs"
version="${CLAUDE_SUBS_VERSION:-latest}"

say() { printf '%s\n' "$*"; }
fail() {
  printf 'claude-subs install: %s\n' "$*" >&2
  exit 1
}

[ -n "${HOME:-}" ] || fail "HOME is not set"
[ "$(uname -s)" = Linux ] || fail "this installer is for Linux (on Windows, build the tray app: see the README)"
case "$(uname -m)" in
  x86_64 | amd64) target=x86_64-unknown-linux-musl ;;
  aarch64 | arm64) target=aarch64-unknown-linux-musl ;;
  *) fail "there is no build for $(uname -m)" ;;
esac

data="${CLAUDE_SUBS_DATA_DIR:-${XDG_DATA_HOME:-$HOME/.local/share}/claude-subs}"
bindir="$HOME/.local/bin"

if [ "${1:-}" = "--uninstall" ]; then
  if [ -x "$data/bin/claude-subs" ]; then
    "$data/bin/claude-subs" setup --remove --mod-dir "$data/mod" || true
  fi
  rm -rf "$data/bin" "$data/mod"
  if [ -L "$bindir/claude-subs" ]; then rm -f "$bindir/claude-subs"; fi
  say "claude-subs removed. Saved logins are still in $data/vault.dat; delete $data to remove them too."
  exit 0
fi

if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 "$1" -o "$2"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  fail "needs curl or wget"
fi
command -v tar >/dev/null 2>&1 || fail "needs tar"
if command -v sha256sum >/dev/null 2>&1; then
  sha256() { sha256sum "$1" | cut -d' ' -f1; }
elif command -v shasum >/dev/null 2>&1; then
  sha256() { shasum -a 256 "$1" | cut -d' ' -f1; }
else
  fail "needs sha256sum"
fi

if [ -n "${CLAUDE_SUBS_BASE_URL:-}" ]; then
  base="$CLAUDE_SUBS_BASE_URL"
elif [ "$version" = latest ]; then
  base="https://github.com/$repo/releases/latest/download"
else
  base="https://github.com/$repo/releases/download/$version"
fi
asset="claude-subs-$target.tar.gz"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading $asset ($version)..."
fetch "$base/$asset" "$tmp/$asset" || fail "could not download $base/$asset"
fetch "$base/$asset.sha256" "$tmp/$asset.sha256" || fail "could not download the checksum"
[ "$(cut -d' ' -f1 <"$tmp/$asset.sha256")" = "$(sha256 "$tmp/$asset")" ] || fail "the download does not match its checksum"

mkdir -p "$tmp/x"
tar -xzf "$tmp/$asset" -C "$tmp/x"
[ -x "$tmp/x/bin/claude-subs" ] && [ -f "$tmp/x/mod/.claude-plugin/plugin.json" ] || fail "the archive is not a claude-subs release"

mkdir -p "$bindir"
umask 077
mkdir -p "$data"
chmod 700 "$data"
# The binary and the mod are replaced whole; the vault and settings beside them stay.
rm -rf "$data/bin.new" "$data/mod.new"
mv "$tmp/x/bin" "$data/bin.new"
mv "$tmp/x/mod" "$data/mod.new"
rm -rf "$data/bin" "$data/mod"
mv "$data/bin.new" "$data/bin"
mv "$data/mod.new" "$data/mod"
chmod 755 "$data/bin/claude-subs"
ln -sf "$data/bin/claude-subs" "$bindir/claude-subs"

say "Installed $("$data/bin/claude-subs" version) in $data"
"$data/bin/claude-subs" setup --mod-dir "$data/mod"
say ""
"$data/bin/claude-subs" status || true
say ""
say "Next:"
say "  * Start a new Claude Code session: the band above the prompt shows your accounts."
say "  * Run /login there once per account; each login is saved on its own."
say "  * Switch with a button in the band, /subs <account>, or: claude-subs switch <account>"
say "  * Never /logout: it revokes the login, the saved copy included."
case ":$PATH:" in
  *":$bindir:"*) ;;
  *) say "  * $bindir is not on your PATH; add it to run claude-subs by name." ;;
esac
