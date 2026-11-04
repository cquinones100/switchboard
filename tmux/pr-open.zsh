#!/bin/zsh

# Popup for the M-p palette's "Open a PR in the browser": asks for a pull request number or URL and opens it
# in the browser with `gh pr view --web`.
#
# Usage: pr-open.zsh
#
# Ctrl-T in the field opens the PR for the focused pane's branch instead.
#
# A URL names its repository, so it works from anywhere. A bare number is
# looked up in the repository the popup starts in, which is the focused pane's
# directory (the binding's -d). When the clipboard already holds a PR URL or
# number, the field starts with it, since that is usually where it came from.
#
# zsh for vared, which edits a prefilled line.

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${0:A:h}/count-request.sh" 2>/dev/null || count_request() { :; }
dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'

# Esc cancels; KEYTIMEOUT=1 keeps it from waiting for an escape sequence.
KEYTIMEOUT=1
bindkey -e
bindkey '\e' send-break

# Ctrl-T ("this tab") opens the PR for the focused pane's branch, what M-r
# does, whatever is in the field. The popup starts in that pane's directory, so
# gh picks the branch from there.
this_tab=false
open-this-tab() { this_tab=true; zle accept-line; }
zle -N open-this-tab
bindkey '^T' open-this-tab

# Errors stay up until a key is pressed; a popup that closed on them would
# look like nothing happened.
fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

pr_url='^https://github\.com/[^/]+/[^/]+/pull/[0-9]+([/?#].*)?$'
pr_number='^#?[0-9]+$'

clipboard=$(pbpaste 2>/dev/null | head -1 | tr -d '[:space:]')
input=
[[ $clipboard =~ $pr_url || $clipboard =~ $pr_number ]] && input=$clipboard

print
print "  ${dim}A PR number or URL, then Enter.   Ctrl-T: this tab's PR   Esc: cancel${reset}"
print
vared -p '  PR: ' input || exit 0
input=${input//[[:space:]]/}

if $this_tab; then
  git rev-parse --git-dir >/dev/null 2>&1 || fail "This tab is not in a repository, so it has no PR."
  print -n "  ${dim}Opening...${reset}"
  count_request GitHub GraphQL 'open a PR' gh
  error=$(gh pr view --web 2>&1 >/dev/null) || fail "No PR for this tab's branch: ${error:-unknown error}"
  exit 0
fi

[[ -z $input ]] && exit 0

if [[ $input =~ $pr_number ]]; then
  input=${input#\#}
  git rev-parse --git-dir >/dev/null 2>&1 ||
    fail "PR $input needs a repository, and $PWD is not in one. Paste the PR's URL instead."
elif [[ ! $input =~ $pr_url ]]; then
  fail "Not a PR number or GitHub PR URL: $input"
fi

print -n "  ${dim}Opening...${reset}"
count_request GitHub GraphQL 'open a PR' gh
error=$(gh pr view "$input" --web 2>&1 >/dev/null) || fail "gh could not open it: ${error:-unknown error}"
