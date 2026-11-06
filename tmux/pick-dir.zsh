#!/bin/zsh

# Asks which directory to open something in, from the ones you work in, and
# prints it. For the popups that open a tab running Claude: new-tab.zsh (M-t,
# M-n) and shape-open.zsh (the palette's "Continue shaping Linear tickets"),
# and, with --repos, the ones that make a worktree: wt-open.zsh (M-W),
# wtb-open.zsh (M-B), wtpr-open.zsh and wtn-open.zsh.
#
# Usage: dir=$(pick-dir.zsh [--repos] "<what Enter does>") || exit 0
#
# An fzf list, the way the Slack relay's places and the Work map find them:
# the directory you are in first (the popup's -d), then every open tab's, then
# every one a Claude session ran in, most recent first, from wt-revive's cache
# (~/.cache/wt-revive/index.json, kept fresh by the sidebar daemon), then your
# home. Only directories that still exist are listed, and no worktree but the
# one you are in. Type to narrow it; Enter takes the chosen one, or, when
# nothing matches, what you typed if it is a directory. Exits 1 on Esc or a
# path that isn't a directory, having said which.
#
# --repos lists git repositories alone, each as its main clone, since a
# worktree is made beside the main clone whichever of its worktrees asks: a
# directory inside one, or one of its worktrees, the one you are in included,
# is its main clone, and anything in no repository, your home among them, is
# left out. A path typed in must be in a repository too.

# Its lists look like mission control's boxes (fzf-style.sh).
export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${0:A:h}/fzf-style.sh" 2>/dev/null)"
repos=
[[ ${1:-} == --repos ]] && { repos=1; shift; }
here=$PWD

# A directory's main clone, or nothing outside a repository: its own
# checkout, or, in a linked worktree, the checkout its git directory sits in,
# so a submodule, whose git directory is not beside it, is its checkout.
main_clone() {
  local out
  out=$(git -C "$1" rev-parse --path-format=absolute --git-common-dir --git-dir --show-toplevel 2>/dev/null) || return 1
  out=("${(@f)out}")
  [[ -n ${out[3]} ]] || return 1
  if [[ ${out[1]} == "${out[2]}" ]]; then print -r -- "${out[3]}"; else print -r -- "${out[1]:h}"; fi
}
typeset -U dirs
dirs=(
  "$here"
  ${(f)"$(tmux list-panes -a -F '#{?@sidebar,,#{pane_current_path}}' 2>/dev/null)"}
  ${(f)"$(jq -r '.transcripts | to_entries | sort_by(-.value.mtimeMs) | .[].value.cwd // empty' ~/.cache/wt-revive/index.json 2>/dev/null)"}
  "$HOME"
)
shown=()
# Worktrees (.claude/worktrees/…) are left out: there are dozens, and a new
# tab belongs in a repository, where wt or wtpr makes its worktree. The one you
# are in stays, first, since that is where you asked from.
typeset -U shown
for dir in $dirs; do
  [[ -n $dir && -d $dir ]] || continue
  if [[ -n $repos ]]; then
    dir=$(main_clone "$dir") || continue
  elif [[ $dir == */.claude/worktrees/* && $dir != "$here" ]]; then
    continue
  fi
  shown+=("${dir/#$HOME/~}")
done

# fzf prints the query, then the choice: none when nothing matches (exit 1),
# and it exits 130 on Esc. Its list is drawn on the terminal, not on stdout.
out=$(print -rl -- $shown | fzf --print-query --no-sort --layout=reverse --height=100% \
  --prompt='Open in > ' --header="${1:-Enter picks it}; a path you type works too. Esc cancels.")
(( $? == 130 )) && exit 1
picked=("${(@f)out}")
choice=${picked[2]:-${picked[1]}}
[[ -n $choice ]] || exit 1
dir=${choice/#\~/$HOME}
if [[ ! -d $dir ]]; then
  print -u2 "\n  \e[31mNo such directory: $choice\e[0m\n\n  \e[38;5;244mPress any key to close.\e[0m"
  read -sk 1
  exit 1
fi
if [[ -n $repos ]] && ! dir=$(main_clone "$dir"); then
  print -u2 "\n  \e[31mNot in a repository: $choice\e[0m\n\n  \e[38;5;244mPress any key to close.\e[0m"
  read -sk 1
  exit 1
fi
print -r -- "$dir"
