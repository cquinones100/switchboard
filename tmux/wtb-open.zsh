#!/bin/zsh

# Popup for M-B, "Start work on a branch, new or existing, in a new tab
# (wtb)": an fzf list of the repository's branches, local and remote, most
# recently committed first, and runs `wtb` with the one picked in a new tab, the
# way wtpr-open.zsh runs `wtpr` for PRs. Typing a name no branch matches and
# pressing Enter creates it; ctrl-n creates the name typed even when branches
# match it.
#
# Usage: wtb-open.zsh
#
# The tab starts in the focused pane's directory (the binding's -d), so the
# worktree is made in that repository. The list is read from the refs already
# here, with no fetch, so a branch pushed since the last fetch is typed by name.
# `wtb` is typed into the new tab's interactive shell rather than run as its
# command, as wtpr-open.zsh does: the shell is what defines it, from ~/.zshrc,
# and it stays in the worktree after Claude exits. So only a name git accepts
# as a branch is ever typed.

# Its lists look like mission control's boxes (fzf-style.sh).
export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${0:A:h}/fzf-style.sh" 2>/dev/null)"
dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'

fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

git rev-parse --git-dir >/dev/null 2>&1 || fail "This tab is not in a repository. Run it from the repo the branch belongs to."

# Each branch once, under its own name: a remote's copy of a local branch, and
# the remote's HEAD, are left out.
branches=$(
  git for-each-ref --sort=-committerdate --format=$'%(refname)\t%(committerdate:relative)' refs/heads refs/remotes |
    awk -F'\t' -v dim="$dim" -v reset="$reset" '
      { name = $1; sub(/^refs\/heads\//, "", name); sub(/^refs\/remotes\/[^\/]+\//, "", name) }
      $1 ~ /\/HEAD$/ || seen[name]++ { next }
      { printf "%s\t%s%s%s\n", name, dim, $2, reset }'
)

# --print-query puts what was typed on the first line and the branch picked,
# if any, on the second; ctrl-n prints the first line alone. fzf exits 1 for
# Enter with nothing matched, which is the name to create, and 130 for Esc.
out=$(
  print -r -- "$branches" |
    fzf --ansi --delimiter=$'\t' --nth=1 --print-query --bind 'ctrl-n:print-query' \
      --header='Enter: work on the branch · a name no branch matches is created · ctrl-n: create the name typed' \
      --prompt='branch> ' --layout=reverse --height=100%
)
(( $? > 1 )) && exit 0
picked=("${(@f)out}")
input=${${picked[2]:-$picked[1]}%%$'\t'*}
input=${input//[[:space:]]/}
[[ -n $input ]] || exit 0

# Also keeps anything the shell would read specially out of what is typed.
[[ $input =~ '^[A-Za-z0-9._/-]+$' ]] && git check-ref-format --branch "$input" >/dev/null 2>&1 ||
  fail "Not a branch name git accepts: $input"

window=$(tmux new-window -c "$PWD" -n "$input" -P -F '#{window_id}') || fail "tmux could not open a tab."
tmux send-keys -t "$window" "wtb $input" Enter
