#!/usr/bin/env bash
# Clears what M-d's diff keeps of a tab's branch, so its next opening asks
# again: Haiku's order of its files and its story, found by a headless
# Neovim opening the same diff M-d would (diff-base.sh's base, nvim/diff.lua)
# with g:diff_clear (lua/plugins/diffview.lua's `clear`), and the PR base
# kept for the worktree (diff-base.sh). Nothing is told again until M-d. A
# story being told right now is kept, since its telling would keep it again.
# M-p's "Clear this tab's diff cache" runs it once confirm.sh has had a yes;
# it runs in that popup, so it says so after (-a).
#
# Usage: diff-cache-clear.sh [<dir>]   the pane's directory, $PWD by default

set -u
dir=${1:-$PWD}
here=${0%/*}
say() { "$here/say.sh" -a "$1"; }

gitdir=$(git -C "$dir" rev-parse --absolute-git-dir 2>/dev/null) || { say "Not in a git repository, so there is no diff cache here."; exit 0; }
top=$(git -C "$dir" rev-parse --show-toplevel)

# The base asked of GitHub only with the github setting on; without it,
# only what is kept for a base still cached is found.
if ! ref=$("$here/diff-base.sh" cached "$top"); then
  ref=
  [ "$(tmux show -gqv @setting_github 2>/dev/null)" = off ] || ref=$("$here/diff-base.sh" fetch "$top")
fi
out= missed=
if ! [[ $ref =~ ^[A-Za-z0-9._/-]+$ ]]; then
  missed="its PR base couldn't be worked out"
elif ! command -v nvim >/dev/null; then
  missed="Neovim isn't installed"
else
  out=$(cd "$top" && nvim --headless -u "$HOME/.config/nvim/diff.lua" --noplugin --cmd 'let g:diff_clear = 1' \
    -c "DiffviewOpen $ref...HEAD" -c "lua vim.defer_fn(function() vim.cmd('cquit 2') end, 30000)" </dev/null 2>/dev/null | grep -o 'cleared [0-9]*\( telling\)\{0,1\}' | tail -1)
  [ -n "$out" ] || missed="Neovim couldn't open its diff"
fi
removed=$(printf '%s' "$out" | tr -dc 0-9)
removed=${removed:-0}
base="${XDG_CACHE_HOME:-$HOME/.cache}/switchboard/diff-base/$(printf '%s' "$gitdir" | tr / %)"
based=
[ -f "$base" ] && rm -f "$base" && based=1 && removed=$((removed + 1))

if [ -n "$missed" ]; then
  text="Couldn't clear this tab's diff story and file order: $missed"
  [ -n "$based" ] && text="$text. Its PR base is cleared." || text="$text."
else
  case $removed in
    0) text="Nothing kept for this tab's diff" ;;
    1) text="This tab's diff cache is cleared: 1 file" ;;
    *) text="This tab's diff cache is cleared: $removed files" ;;
  esac
  case $out in
    *telling) text="$text. Its story is being told right now, so it was kept: clear it again once M-d shows it." ;;
    *) text="$text." ;;
  esac
fi
say "$text"
