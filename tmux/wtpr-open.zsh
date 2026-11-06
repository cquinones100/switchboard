#!/bin/zsh

# Popup for the M-p palette's "Start work on a PR in a new tab (wtpr)": asks
# for a pull request number or URL and runs `wtpr` with it in a new tab, the
# way wt-open.zsh runs `wt` for tickets.
#
# Usage: wtpr-open.zsh
#
# A bare number then asks which repository it is a PR of (pick-dir.zsh), the
# focused pane's (the binding's -d) at the top, and the tab starts there; a
# URL names its own repository, which wtpr finds a clone of, so it asks
# nothing more and the tab starts in the focused pane's directory. `wtpr` is typed
# into the new tab's interactive shell rather than run as its command, as
# wt-open.zsh does with `wt`: the shell is what defines it, from ~/.zshrc, and
# it stays in the worktree after Claude exits. So only a number, or a URL
# rebuilt from its owner, name and number, is ever typed.
#
# The field starts with the clipboard when it holds a PR URL or number. zsh for
# vared, which edits a prefilled line.

dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'

# Esc cancels; KEYTIMEOUT=1 keeps it from waiting for an escape sequence.
KEYTIMEOUT=1
bindkey -e
bindkey '\e' send-break

fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

pr_url='^https://github\.com/([A-Za-z0-9._-]+)/([A-Za-z0-9._-]+)/pull/([0-9]+)([/?#].*)?$'
pr_number='^#?([0-9]+)$'

clipboard=$(pbpaste 2>/dev/null | head -1 | tr -d '[:space:]')
input=
[[ $clipboard =~ $pr_url || $clipboard =~ $pr_number ]] && input=$clipboard

print
print "  ${dim}A PR number or URL, then Enter, to work on it in a new tab. Esc cancels.${reset}"
print
vared -p '  PR: ' input || exit 0
input=${input//[[:space:]]/}
[[ -n $input ]] || exit 0

if [[ $input =~ $pr_url ]]; then
  pr="https://github.com/$match[1]/$match[2]/pull/$match[3]"
elif [[ $input =~ $pr_number ]]; then
  pr=$match[1]
  dir=$("${0:A:h}/pick-dir.zsh" --repos "PR #$pr of this repository: Enter works on it in a new tab") || exit 0
  cd "$dir" || exit 0
else
  fail "Not a PR number or GitHub PR URL: $input"
fi

window=$(tmux new-window -c "${PWD//\#/##}" -n "PR ${pr##*/}" -P -F '#{window_id}') || fail "tmux could not open a tab."
tmux send-keys -t "$window" "wtpr $pr" Enter
