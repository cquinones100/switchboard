#!/bin/sh
# MIT License
#
# Copyright (c) 2026 Carlos Quinones
#
# Permission is hereby granted, free of charge, to any person obtaining a copy
# of this software and associated documentation files (the "Software"), to deal
# in the Software without restriction, including without limitation the rights
# to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
# copies of the Software, and to permit persons to whom the Software is
# furnished to do so, subject to the following conditions:
#
# The above copyright notice and this permission notice shall be included in all
# copies or substantial portions of the Software.
#
# THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
# IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
# FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
# AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
# LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
# OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
# SOFTWARE.
#
# SPDX-License-Identifier: MIT
#
# Switchboard's install, for someone new. Each release carries the
# install.sh of the commit it was made from, signed with the release key
# beside its archives, so the installer run is the one a release was made
# with, never whatever main holds. The README's Install section downloads
# it with its signature (install.sh.sig), checks that against the release
# key below, and only then runs it; then `switchboard setup`, as the last
# line this prints says.
#
# It finds the newest release for this computer (x86_64 Linux, or an Intel
# or Apple silicon Mac), downloads its archive over HTTPS with the SHA-256
# and the signature published beside it, refuses the archive unless the
# checksum agrees and the signature is by Switchboard's release key (whose
# public half is below, so a release changed after it was signed is refused
# even with its checksum changed to match), and puts `switchboard` in
# ~/.local/bin, saying how to add that to your PATH when it isn't there.
# It then says to run `switchboard setup`, the guided setup, as a step of
# its own (SWITCHBOARD_SETUP=1 runs it here instead). With `--diff` it installs `switchboard-diff` instead,
# the diff alone, and with `--notes` `switchboard-notes`, Notes alone: each
# has releases of its own in the same repository, tagged `diff-v<version>`
# and `notes-v<version>`, signed with the same key. Run again, it installs
# the newest release over the one there; `switchboard update` does the same
# from then on.
#
# It needs only what macOS and common Linux distributions ship: a POSIX sh,
# curl or wget, tar, shasum or sha256sum, and OpenSSH's ssh-keygen (8.1 or
# newer, from 2019).
#
# What changes it (for a release other than the newest, or for the tests):
#   SWITCHBOARD_VERSION      a release's tag, as v0.2.0 (diff-v0.2.0 with
#                            --diff, notes-v0.2.0 with --notes), in place
#                            of the newest
#   SWITCHBOARD_BIN_DIR      where the binary goes, in place of ~/.local/bin
#   SWITCHBOARD_GITHUB_API   GitHub's API, in place of https://api.github.com
#   SWITCHBOARD_SETUP        1, it runs `switchboard setup` once installed
#   SWITCHBOARD_RELEASE_KEY  a public key trusted beside the release keys, for
#                            the tests' own releases
#
#   sh install.sh --target   prints the release target for this computer and
#                            stops, changing nothing
#   sh install.sh --diff     installs `switchboard-diff` instead, the diff as
#                            an app of its own (crates/diff/README.md), from
#                            its own newest release (tagged `diff-v<version>`),
#                            checked the same way, with no setup to run
#   sh install.sh --notes    installs `switchboard-notes` instead, Notes as
#                            an app of its own (crates/notes/README.md), from
#                            its own newest release (tagged
#                            `notes-v<version>`), the same way
#
# Everything is in functions called from the last line, so the whole script
# is read before any of it runs: piped into sh, the rest of the script is
# still on its input while it runs, and nothing it starts may read it.

set -eu

# The repository releases come from; src/update/release.rs's REPO.
REPO=cquinones100/switchboard

# The public halves of the keys a release may be signed with, one OpenSSH
# key a line; src/update/release-signing-keys.pub's, which the binary
# carries, and the namespace its signatures are made for.
RELEASE_KEYS='ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIHk/NIR8Ueq68RDwrJ3vWpLvcNX85BrMHDf0UBVnk05B switchboard-release'
NAMESPACE=switchboard-release

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
# curl, else wget, failing on an HTTP error: only from an HTTPS address, and
# with curl only through HTTPS redirects, or from this computer's own (the
# tests' stand-ins).
fetch() {
  case $1 in
    *@*) fail "won't download $1, which names a user." ;;
    https://*) proto='=https' ;;
    http://127.0.0.1:* | http://127.0.0.1/* | http://localhost:*) proto='=http,https' ;;
    *) fail "won't download $1, which isn't an HTTPS address." ;;
  esac
  if command -v curl >/dev/null 2>&1; then
    if [ $# -gt 1 ]; then
      curl -fsSL --retry 2 --proto "$proto" --proto-redir "$proto" -o "$2" "$1"
    else
      curl -fsSL --retry 2 --proto "$proto" --proto-redir "$proto" "$1"
    fi
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

# Refuses the archive at $1 unless $2, its signature as `ssh-keygen -Y
# sign` writes it, is by one of RELEASE_KEYS (or SWITCHBOARD_RELEASE_KEY).
check_signature() {
  command -v ssh-keygen >/dev/null 2>&1 \
    || fail "ssh-keygen (OpenSSH's) isn't installed, so the download's signature can't be checked."
  keys=$RELEASE_KEYS
  if [ -n "${SWITCHBOARD_RELEASE_KEY:-}" ]; then
    keys="$keys
$SWITCHBOARD_RELEASE_KEY"
  fi
  printf '%s\n' "$keys" \
    | awk -v ns="$NAMESPACE" 'NF >= 2 { printf "switchboard-release namespaces=\"%s\" %s %s\n", ns, $1, $2 }' \
    > "$tmp/allowed_signers"
  ssh-keygen -Y verify -f "$tmp/allowed_signers" -I switchboard-release -n "$NAMESPACE" \
    -s "$2" < "$1" > /dev/null 2>&1 \
    || fail "$(basename "$1") isn't signed by Switchboard's release key, so it may not be the file that was published (or this ssh-keygen is older than OpenSSH 8.1). Nothing was installed."
}

# A string field of GitHub's JSON for a release, one value a line. The JSON
# is split at its commas first, since GitHub may send it on one line; no
# value read here (a tag, a download URL) has a comma in it.
field() {
  tr ',' '\n' | sed -n "s/.*\"$1\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p"
}

main() {
  # The program installed, and what it is called when said.
  app=switchboard
  called=Switchboard
  # What a release's tag starts with before its version.
  tagged=v
  case ${1:-} in
    --target)
      target
      return
      ;;
    --diff)
      app=switchboard-diff
      called="Switchboard's diff"
      tagged=diff-v
      ;;
    --notes)
      app=switchboard-notes
      called="Switchboard's Notes"
      tagged=notes-v
      ;;
    "") ;;
    *) fail "it takes --target, --diff or --notes, not \"$1\"." ;;
  esac
  api=${SWITCHBOARD_GITHUB_API:-https://api.github.com}
  api=${api%/}
  bin_dir=${SWITCHBOARD_BIN_DIR:-$HOME/.local/bin}
  t=$(target)

  if [ -n "${SWITCHBOARD_VERSION:-}" ]; then
    url="$api/repos/$REPO/releases/tags/$SWITCHBOARD_VERSION"
  elif [ "$app" = switchboard ]; then
    url="$api/repos/$REPO/releases/latest"
  else
    # The repository's latest release is Switchboard's, so the newest of the
    # diff or of Notes is found in the list, which GitHub gives newest first, a hundred a
    # page: read page by page until one is there or a page comes short,
    # which is the last. Only a plain version counts (diff-v1.2.3), as
    # GitHub's latest leaves pre-releases out: one tagged diff-v1.3.0-rc.1
    # is installed only when SWITCHBOARD_VERSION names it.
    newest=
    page=1
    while [ -z "$newest" ] && [ "$page" -le 50 ]; do
      list=$(fetch "$api/repos/$REPO/releases?per_page=100&page=$page") \
        || fail "couldn't read the releases from GitHub ($api/repos/$REPO/releases)."
      tags=$(printf '%s\n' "$list" | field tag_name)
      newest=$(printf '%s\n' "$tags" | grep -E "^$tagged[0-9]+\.[0-9]+\.[0-9]+\$" | head -n 1 || true)
      [ "$(printf '%s\n' "$tags" | grep -c .)" -ge 100 ] || break
      page=$((page + 1))
    done
    [ -n "$newest" ] || fail "there is no release of $app yet; build it from source with cargo."
    url="$api/repos/$REPO/releases/tags/$newest"
  fi
  json=$(fetch "$url") || fail "couldn't read the release from GitHub ($url)."
  tag=$(printf '%s\n' "$json" | field tag_name | head -n 1)
  [ -n "$tag" ] || fail "GitHub's answer named no release."
  case $tag in
    "$tagged"*) ;;
    *) fail "release $tag isn't one of $app." ;;
  esac
  version=${tag#"$tagged"}
  archive="$app-$version-$t.tar.gz"
  archive_url=$(printf '%s\n' "$json" | field browser_download_url | grep "/$archive\$" | head -n 1 || true)
  sum_url=$(printf '%s\n' "$json" | field browser_download_url | grep "/$archive.sha256\$" | head -n 1 || true)
  sig_url=$(printf '%s\n' "$json" | field browser_download_url | grep "/$archive.sig\$" | head -n 1 || true)
  [ -n "$archive_url" ] && [ -n "$sum_url" ] && [ -n "$sig_url" ] \
    || fail "release $tag has no $archive with its checksum and signature."

  tmp=$(mktemp -d 2>/dev/null || mktemp -d -t switchboard)
  trap 'rm -rf "$tmp"' EXIT
  trap 'exit 130' INT TERM
  say "Downloading $called $version for ${t}…"
  fetch "$archive_url" "$tmp/$archive" || fail "couldn't download $archive."
  fetch "$sum_url" "$tmp/$archive.sha256" || fail "couldn't download $archive.sha256."
  fetch "$sig_url" "$tmp/$archive.sig" || fail "couldn't download $archive.sig."

  # The checksum file is `<hash>  <name>`, as shasum writes it.
  expected=$(sed -n '1s/^[[:space:]]*\([0-9A-Fa-f]\{64\}\).*/\1/p' "$tmp/$archive.sha256" | tr 'A-F' 'a-f')
  [ -n "$expected" ] || fail "$archive.sha256 doesn't hold a SHA-256."
  actual=$(sha256 "$tmp/$archive")
  [ "$actual" = "$expected" ] \
    || fail "$archive isn't the file that was published (its SHA-256 is $actual, the release says $expected). Nothing was installed."
  check_signature "$tmp/$archive" "$tmp/$archive.sig"
  say "Checked its checksum, and its signature by Switchboard's release key."

  mkdir "$tmp/unpacked"
  tar -xzf "$tmp/$archive" -C "$tmp/unpacked" || fail "couldn't unpack $archive."
  binary=$(find "$tmp/unpacked" -type f -name "$app" | head -n 1)
  [ -n "$binary" ] || fail "$archive has no $app in it."

  # Copied beside the old one and renamed over it, so a switchboard running
  # now goes on running, and nothing half-copied is ever in its place. It
  # is run once first, named switchboard in a folder of its own there: a
  # release up to 0.2.0 reads any other name as a command it doesn't have.
  mkdir -p "$bin_dir" || fail "couldn't make $bin_dir."
  new="$bin_dir/.switchboard.new.$$"
  mkdir "$new" || fail "couldn't write in $bin_dir."
  cp "$binary" "$new/$app" || { rm -rf "$new"; fail "couldn't write in $bin_dir."; }
  chmod 755 "$new/$app"
  if ! started=$("$new/$app" --version 2>&1); then
    rm -rf "$new"
    fail "the $app in $archive won't start on this computer ($started). Nothing was installed."
  fi
  mv -f "$new/$app" "$bin_dir/$app"
  rmdir "$new"
  say "Installed $("$bin_dir/$app" --version) in $bin_dir."

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

  # The diff alone has nothing to set up: it runs in any repository. Nor
  # has Notes alone: it opens the notes in ~/Notes.
  if [ "$app" = switchboard-diff ]; then
    say ""
    say "Run it in a repository: switchboard-diff"
    return 0
  fi
  if [ "$app" = switchboard-notes ]; then
    say ""
    say "Run it on the notes in ~/Notes: switchboard-notes"
    return 0
  fi
  say ""
  if [ "${SWITCHBOARD_SETUP:-}" != 1 ]; then
    # By the path just installed, so neither another switchboard first on
    # the PATH nor a folder off it runs; quoted when the shell would split
    # it.
    setup="$bin_dir/switchboard"
    case $setup in
      *[!A-Za-z0-9/._~+-]*) setup="'$(printf '%s' "$setup" | sed "s/'/'\\\\''/g")'" ;;
    esac
    setup="$setup setup"
    say "Next, set it up: $setup"
    return 0
  fi
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
