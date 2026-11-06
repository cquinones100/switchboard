#!/usr/bin/env bash
# Tells a branch's diff story ahead of time, so M-d opens on it at once: a
# headless Neovim opens the same diff M-d would (diff-base.sh's base, the
# same diffview config, nvim/diff.lua), which asks Haiku for the story and
# keeps it in Neovim's cache by the diff's own text, then quits (g:diff_ahead,
# lua/plugins/diffview.lua). Run by the sidebar daemon in the background when
# a tab's branch has a commit it hasn't told yet (lib/sidebar/story-ahead.js),
# with the diff_story and diff_story_ahead settings on.
#
# Usage: diff-story-ahead.sh <dir>
#
# Once per commit: the commit told is kept per worktree, and a second run on
# the same one, or while one runs, does nothing.

set -u
dir=${1:-}
here=${0%/*}
[ -n "$dir" ] || exit 1
facts=$(git -C "$dir" rev-parse --absolute-git-dir --show-toplevel HEAD 2>/dev/null) || exit 0
gitdir=$(printf '%s\n' "$facts" | sed -n 1p)
top=$(printf '%s\n' "$facts" | sed -n 2p)
head=$(printf '%s\n' "$facts" | sed -n 3p)
state="${XDG_STATE_HOME:-$HOME/.local/state}/switchboard/diff-story-ahead"
key=$(printf '%s' "$gitdir" | tr / %)
mkdir -p "$state"
[ "$(cat "$state/$key" 2>/dev/null)" = "$head" ] && exit 0
# One at a time per worktree.
mkdir "$state/$key.lock" 2>/dev/null || exit 0
trap 'rmdir "$state/$key.lock" 2>/dev/null' EXIT

# A base that couldn't be fetched isn't the one M-d will compare with once
# it can: left to try again rather than told against the old one.
if ! ref=$("$here/diff-base.sh" cached "$top"); then
  ref=$("$here/diff-base.sh" fetch "$top") || exit 3
fi
[[ $ref =~ ^[A-Za-z0-9._/-]+$ ]] || exit 0
command -v nvim >/dev/null || exit 0
# At most five and a half minutes, as long as the story's own wait and a bit.
# Exits 0 once the story is told (or there was none to tell), else not, and
# then the commit is left untold, to be tried again, with why on stderr (the
# last line Neovim wrote there, story-ahead.js's log). A story told with a
# warning (its answer mostly named hunks the diff doesn't have) exits 0 with
# the warning on stderr, which the log keeps too.
why=$(cd "$top" && nvim --headless -u "$HOME/.config/nvim/diff.lua" --noplugin --cmd 'let g:diff_ahead = 1' \
  -c "DiffviewOpen $ref...HEAD" -c "lua vim.defer_fn(function() io.stderr:write('Neovim gave up waiting after 330 seconds\\n') vim.cmd('cquit 2') end, 330000)" </dev/null 2>&1 >/dev/null) || {
  printf '%s\n' "$why" | grep -v '^[[:space:]]*$' | tail -1 >&2
  exit 2
}
printf '%s\n' "$why" | grep "^Haiku's answer named hunks" | tail -1 >&2
printf '%s\n' "$head" >"$state/$key"
