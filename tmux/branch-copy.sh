#!/usr/bin/env bash
# Copies something about the current branch to the clipboard, and says what.
# Usage: branch-copy.sh <ticket|ticket-url|pr|pr-url> <pane_current_path>
#
#   ticket      the Linear ticket ID the branch names, e.g. ABC-123
#   ticket-url  that ticket's Linear URL
#   pr          the number of the branch's pull request, e.g. 123
#   pr-url      that pull request's URL
#
# The ticket comes from the branch name and the Linear workspace from config,
# as linear-ticket.sh finds them, so no network is needed. The PR comes from
# gh, which takes a second; the binding runs this in the background meanwhile.

what=$1
cd "$2" 2>/dev/null || exit 0

say() { "${0%/*}/say.sh" "$1"; }

case "$what" in
  ticket | ticket-url)
    # Any team key, as linear-ticket.sh matches it.
    ticket=$(git branch --show-current 2>/dev/null |
      grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
    [ -n "$ticket" ] || { say "This branch doesn't name a ticket."; exit 0; }
    value=$ticket
    if [ "$what" = ticket-url ]; then
      config="$HOME/.config/switchboard/linear.json"
      workspace="${LINEAR_WORKSPACE:-$(jq -r '.linearWorkspace // empty' "$config" 2>/dev/null)}"
      [ -n "$workspace" ] || { say "No Linear workspace set: add linearWorkspace to $config."; exit 0; }
      value="https://linear.app/$workspace/issue/$ticket"
    fi
    ;;
  pr | pr-url)
    field=$([ "$what" = pr ] && echo number || echo url)
    value=$(gh pr view --json "$field" --jq ".$field" 2>/dev/null)
    [ -n "$value" ] || { say "This branch has no pull request."; exit 0; }
    ;;
  *)
    say "branch-copy.sh: nothing called $what to copy."
    exit 0
    ;;
esac

printf '%s' "$value" | pbcopy
say "Copied $value"
