#!/bin/sh
# Switchboard's one-line install, for someone new:
#
#   curl -fsSL https://raw.githubusercontent.com/cooljacob204/switchboard/main/install.sh | sh
#
# It finds the newest release for this computer (x86_64 Linux, or an Intel
# or Apple silicon Mac), downloads its archive and the SHA-256 published
# beside it, refuses the archive unless the two agree, and puts `switchboard`
# in ~/.local/bin, saying how to add that to your PATH when it isn't there.
# Then it runs `switchboard setup`, the guided setup, which asks before it
# changes anything. Run again, it installs the newest release over the one
# there; `switchboard update` does the same from then on.
#
# It needs only what macOS and common Linux distributions ship: a POSIX sh,
# curl or wget, tar, and shasum or sha256sum.
#
# What changes it (for a release other than the newest, or for the tests):
#   SWITCHBOARD_VERSION      a release's tag, as v0.2.0, in place of the newest
#   SWITCHBOARD_BIN_DIR      where the binary goes, in place of ~/.local/bin
#   SWITCHBOARD_GITHUB_API   GitHub's API, in place of https://api.github.com
#   SWITCHBOARD_NO_SETUP     set, the setup isn't run
#
#   sh install.sh --target   prints the release target for this computer and
#                            stops, changing nothing
#
# Everything is in functions called from the last line, so the whole script
# is read before any of it runs: piped into sh, the rest of the script is
# still on its input while it runs, and nothing it starts may read it.

set -eu

# The repository releases come from; src/update/release.rs's REPO.
REPO=cooljacob204/switchboard

say() { printf '%s\n' "$*"; }
fail() {
  printf 'switchboard install: %s\n' "$*" >&2
  exit 1
}

# The target a release is built for, from uname: Linux on x86_64, and macOS
# on either. A shell under Rosetta on an Apple silicon Mac reads x86_64, so
# the Mac is asked whether it is translating, and gets the native build.
target() {
  os=$(uname -s)
  arch=$(uname -m)
  case $os in
    Linux)
      case $arch in
        x86_64 | amd64) echo x86_64-unknown-linux-gnu ;;
        *) fail "there is no release for Linux on $arch yet; build it from source with cargo." ;;
      esac
      ;;
    Darwin)
      if [ "$arch" = x86_64 ] && [ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
        arch=arm64
      fi
      case $arch in
        arm64 | aarch64) echo aarch64-apple-darwin ;;
        x86_64) echo x86_64-apple-darwin ;;
        *) fail "there is no release for macOS on $arch." ;;
      esac
      ;;
    *) fail "there is no release for $os; Switchboard runs on Linux and macOS." ;;
  esac
}

# A URL's body on stdout (or into a file, with a second argument), with
# curl, else wget, failing on an HTTP error.
fetch() {
  if command -v curl >/dev/null 2>&1; then
    if [ $# -gt 1 ]; then curl -fsSL --retry 2 -o "$2" "$1"; else curl -fsSL --retry 2 "$1"; fi
  elif command -v wget >/dev/null 2>&1; then
    if [ $# -gt 1 ]; then wget -q -O "$2" "$1"; else wget -q -O - "$1"; fi
  else
    fail "neither curl nor wget is installed; install one and run this again."
  fi
}

# A file's SHA-256, in lower case.
sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sum=$(sha256sum "$1")
  elif command -v shasum >/dev/null 2>&1; then
    sum=$(shasum -a 256 "$1")
  else
    fail "neither sha256sum nor shasum is installed, so the download can't be checked."
  fi
  printf '%s\n' "${sum%% *}" | tr 'A-F' 'a-f'
}

# A string field of GitHub's JSON for a release, one value a line. The JSON
# is split at its commas first, since GitHub may send it on one line; no
# value read here (a tag, a download URL) has a comma in it.
field() {
  tr ',' '\n' | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p"
}

main() {
  if [ "${1:-}" = --target ]; then
    target
    return
  fi
  api=${SWITCHBOARD_GITHUB_API:-https://api.github.com}
  api=${api%/}
  bin_dir=${SWITCHBOARD_BIN_DIR:-$HOME/.local/bin}
  t=$(target)

  if [ -n "${SWITCHBOARD_VERSION:-}" ]; then
    url="$api/repos/$REPO/releases/tags/$SWITCHBOARD_VERSION"
  else
    url="$api/repos/$REPO/releases/latest"
  fi
  json=$(fetch "$url") || fail "couldn't read the release from GitHub ($url)."
  tag=$(printf '%s\n' "$json" | field tag_name | head -n 1)
  [ -n "$tag" ] || fail "GitHub's answer named no release."
  version=${tag#v}
  archive="switchboard-$version-$t.tar.gz"
  archive_url=$(printf '%s\n' "$json" | field browser_download_url | grep "/$archive\$" | head -n 1 || true)
  sum_url=$(printf '%s\n' "$json" | field browser_download_url | grep "/$archive.sha256\$" | head -n 1 || true)
  [ -n "$archive_url" ] && [ -n "$sum_url" ] \
    || fail "release $tag has no $archive with its checksum."

  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t switchboard)
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 130' INT TERM
  say "Downloading Switchboard $version for ${t}…"
  fetch "$archive_url" "$tmp/$archive" || fail "couldn't download $archive."
  fetch "$sum_url" "$tmp/$archive.sha256" || fail "couldn't download $archive.sha256."

  # The checksum file is `<hash>  <name>`, as shasum writes it.
  expected=$(sed -n '1s/^[[:space:]]*\([0-9A-Fa-f]\{64\}\).*/\1/p' "$tmp/$archive.sha256" | tr 'A-F' 'a-f')
  [ -n "$expected" ] || fail "$archive.sha256 doesn't hold a SHA-256."
  actual=$(sha256 "$tmp/$archive")
  [ "$actual" = "$expected" ] \
    || fail "$archive isn't the file that was published (its SHA-256 is $actual, the release says $expected). Nothing was installed."
  say "Checked it against $archive.sha256."

  mkdir "$tmp/unpacked"
  tar -xzf "$tmp/$archive" -C "$tmp/unpacked" || fail "couldn't unpack $archive."
  binary=$(find "$tmp/unpacked" -type f -name switchboard | head -n 1)
  [ -n "$binary" ] || fail "$archive has no switchboard in it."

  # Copied beside the old one and renamed over it, so a switchboard running
  # now goes on running, and nothing half-copied is ever in its place.
  mkdir -p "$bin_dir" || fail "couldn't make $bin_dir."
  cp "$binary" "$bin_dir/.switchboard.new.$$" || fail "couldn't write in $bin_dir."
  chmod 755 "$bin_dir/.switchboard.new.$$"
  if ! "$bin_dir/.switchboard.new.$$" --version >/dev/null 2>&1; then
    rm -f "$bin_dir/.switchboard.new.$$"
    fail "the switchboard in $archive won't start on this computer. Nothing was installed."
  fi
  mv -f "$bin_dir/.switchboard.new.$$" "$bin_dir/switchboard"
  say "Installed $("$bin_dir/switchboard" --version) in $bin_dir."

  case ":$PATH:" in
    *":$bin_dir:"*) ;;
    *)
      case ${SHELL:-} in
        */zsh) rc='~/.zshrc' ;;
        */bash) rc='~/.bashrc' ;;
        */fish) rc='~/.config/fish/config.fish' ;;
        *) rc='your shell'"'"'s startup file' ;;
      esac
      say ""
      say "$bin_dir isn't on your PATH. Add this line to $rc, then open a new terminal:"
      if [ "$bin_dir" = "$HOME/.local/bin" ]; then
        say '  export PATH="$HOME/.local/bin:$PATH"'
      else
        say "  export PATH=\"$bin_dir:\$PATH\""
      fi
      ;;
  esac

  [ -z "${SWITCHBOARD_NO_SETUP:-}" ] || return 0
  say ""
  # The setup's questions are read from the terminal: this script's own
  # input is the pipe it came through. With no terminal, its input is
  # closed, which answers every question no.
  if (: </dev/tty) 2>/dev/null; then
    "$bin_dir/switchboard" setup </dev/tty
  else
    "$bin_dir/switchboard" setup </dev/null
  fi
}

main "$@"
