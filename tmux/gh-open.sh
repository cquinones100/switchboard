#!/usr/bin/env bash
# Opens the branch's PR (M-r) or the repository (M-p's "Open the repository")
# on GitHub in the browser, or, when it can't, says why in a box in the
# middle of the screen that any key closes: not a git repository, no remote
# on GitHub, gh missing or signed out, a repository GitHub doesn't have or
# won't show you, or a branch with no PR yet. tmux's own message when a
# command fails says only that it returned 1.
#
# Usage: gh-open.sh <pr|repo> <dir> <client>
#        gh-open.sh box <title> <reason> <hint>   the box itself

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${BASH_SOURCE[0]%/*}/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }
set -u

if [ "${1:-}" = box ]; then
  printf '\033[?25l\n  \033[1m%s\033[0m\n\n' "$3"
  [ -n "${4:-}" ] && printf '  %s\n' "$4"
  printf '\n  \033[38;5;244many key or a click closes\033[0m'
  # A click closes it too.
  printf '\033[?1000h\033[?1006h'
  read -rsn 1
  exit 0
fi

what=${1:-} dir=${2:-} client=${3:-}
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
[ "$what" = pr ] || [ "$what" = repo ] || exit 1

say() {
  local reason=$1 hint=${2:-} title width
  [ "$what" = pr ] && title="#[bold] Can't open the PR " || title="#[bold] Can't open the repository "
  width=$(printf '%s\n%s\n' "$reason" "$hint" | awk '{ if (length > n) n = length } END { print n + 6 }')
  (( width < 40 )) && width=40
  tmux display-popup ${client:+-c "$client"} -E -w "$width" -h 8 -T "$title" \
    "$self box $(printf '%q %q %q' "$title" "$reason" "$hint")"
  exit 0
}

if ! command -v gh >/dev/null; then
  say "gh, GitHub's command line, isn't installed." "brew install gh, then gh auth login"
fi
if ! git -C "$dir" rev-parse --git-dir >/dev/null 2>&1; then
  say "This pane isn't in a git repository." "${dir/#$HOME/~}"
fi
branch=$(git -C "$dir" branch --show-current 2>/dev/null)
remote=$(git -C "$dir" remote get-url origin 2>/dev/null)

if [ "$what" = pr ]; then
  count_request GitHub GraphQL 'open on GitHub' gh
  out=$(cd "$dir" && gh pr view --json url -q .url 2>&1)
else
  count_request GitHub GraphQL 'open on GitHub' gh
  out=$(cd "$dir" && gh repo view --json url -q .url 2>&1)
fi
if [ $? -eq 0 ] && [[ $out == https://* ]]; then
  open "$out"
  exit 0
fi

case $out in
  *"no git remotes"* | *"none of the git remotes"* | *"not a GitHub"*)
    say "This repository has no remote on GitHub." "${remote:-It has no origin remote at all.}" ;;
  *"Could not resolve to a Repository"*)
    say "GitHub has no repository at ${remote:-this remote}," "or you can't see it from the account gh is signed in as." ;;
  *"gh auth login"* | *"authentication"* | *"401"*)
    say "gh isn't signed in to GitHub." "Run gh auth login." ;;
  *"no pull requests found"*)
    say "Branch ${branch:-(none)} has no PR yet." "Push it and open one, or open another PR with M-p." ;;
  *)
    say "GitHub said: $(printf '%s' "$out" | head -1 | cut -c1-70)" "" ;;
esac
