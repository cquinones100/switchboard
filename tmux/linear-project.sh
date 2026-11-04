#!/usr/bin/env bash
# Opens the Linear project of the ticket named in the current branch, or, when
# there is none to open, says why in a popup.
# Usage: linear-project.sh <pane_current_path> <client_name>
#        linear-project.sh --show   (what the popup runs)
#
# Unlike linear-ticket.sh, the URL can't be built from the branch: the project
# is only known to Linear, so this asks its API, with the key worktree-from-ticket
# uses (from $LINEAR_API_KEY, the Keychain or its config file). The binding runs it in the
# background, so the wait never holds up tmux.

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${BASH_SOURCE[0]%/*}/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }
set -u

# What the popup runs: the message, wrapped and indented, until a key is
# pressed. Here rather than in the popup's command, which tmux hands to its
# default shell, not necessarily bash.
if [ "${1:-}" = --show ]; then
  printf '\n'
  printf '%s\n' "$LINEAR_PROJECT_MESSAGE" | fold -s -w 66 | sed 's/^/  /'
  printf '\n  Press any key to close.'
  read -rsn 1
  exit 0
fi

dir="${1:-}"
client="${2:-}"

# A popup on the client the key was pressed in. The text goes in through the
# environment rather than the command, so nothing in it is read by a shell.
explain() {
  "${0%/*}/say.sh" close "$client"
  tmux display-popup -c "$client" -E -w 72 -h 9 -T ' Linear project ' \
    -e "LINEAR_PROJECT_MESSAGE=$1" "$(printf %q "$0") --show"
}

branch=$(git -C "$dir" branch --show-current 2>/dev/null)
[ -n "$branch" ] || { explain "This pane isn't on a git branch, so there's no ticket to find a project for."; exit 0; }

# Matches any team key, the same way linear-ticket.sh does.
ticket=$(printf '%s' "$branch" | grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
[ -n "$ticket" ] || { explain "The branch $branch doesn't name a Linear ticket, so there's no project to open."; exit 0; }

config="$HOME/.config/switchboard/linear.json"
# The macOS Keychain (service linear-api-key), else the config file's linearApiKey.
key="${LINEAR_API_KEY:-$(security find-generic-password -s linear-api-key -w 2>/dev/null || jq -r '.linearApiKey // empty' "$config" 2>/dev/null)}"
[ -n "$key" ] || { explain "There's no Linear API key: set LINEAR_API_KEY, or store it in the Keychain as linear-api-key."; exit 0; }

"${0%/*}/say.sh" -c "$client" "Looking up the Linear project for $ticket..."

body=$(jq -nc --arg id "$ticket" \
  '{query: "query($id: String!) { issue(id: $id) { project { name url } } }", variables: {id: $id}}')
# The key goes to curl on stdin (-H @-), as in ticket-title.sh, so it is never
# on a command line where ps could show it.
count_request Linear GraphQL 'open the ticket'"'"'s project' curl
response=$(printf 'Authorization: %s\n' "$key" |
  curl -s --max-time 10 https://api.linear.app/graphql \
    -H 'Content-Type: application/json' -H @- -d "$body") ||
  { explain "Linear didn't answer, so the project for $ticket couldn't be looked up. Try again in a moment."; exit 0; }

if printf '%s' "$response" | jq -e '.errors[]? | select(.extensions.code == "INPUT_ERROR")' >/dev/null 2>&1; then
  explain "Linear has no ticket $ticket, so there's no project to open."
  exit 0
fi
if ! printf '%s' "$response" | jq -e '.data.issue' >/dev/null 2>&1; then
  explain "Linear couldn't be asked about $ticket: $(printf '%s' "$response" | jq -r '.errors[0].message // "no answer"' 2>/dev/null)"
  exit 0
fi

url=$(printf '%s' "$response" | jq -r '.data.issue.project.url // empty')
[ -n "$url" ] || { explain "$ticket isn't in a Linear project."; exit 0; }

open "$url"
