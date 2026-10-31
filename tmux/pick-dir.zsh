#!/bin/zsh

# Asks which directory to open something in, from the ones you work in, and
# prints it. For the popups that open a tab running Claude: new-tab.zsh (M-t,
# M-n) and shape-open.zsh (the palette's "Continue shaping tickets").
#
# Usage: dir=$(pick-dir.zsh "<what Enter does>") || exit 0
#
# An fzf list, the way the Slack relay's places and the Work map find them:
# the directory you are in first (the popup's -d), then every open tab's, then
# every one a Claude session ran in, most recent first, from wt-revive's cache
# (~/.cache/wt-revive/index.json, kept fresh by the sidebar daemon), then your
# home. Only directories that still exist are listed, and no worktree but the
# one you are in. Type to narrow it; Enter takes the chosen one, or, when
# nothing matches, what you typed if it is a directory. Exits 1 on Esc or a
# path that isn't a directory, having said which.

here=$PWD
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
for dir in $dirs; do
  [[ $dir == */.claude/worktrees/* && $dir != "$here" ]] && continue
  [[ -n $dir && -d $dir ]] && shown+=("${dir/#$HOME/~}")
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
print -r -- "$dir"
