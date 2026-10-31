#!/usr/bin/env bash
# Opens the Linear project, or the Jira epic, of the ticket named in the
# current branch, as the tracker setting says, or, when there is none to open,
# says why in a popup. (Named from when Linear was the only tracker.)
# Usage: linear-project.sh <pane_current_path> <client_name>
#        linear-project.sh --show   (what the popup runs)
#
# Unlike linear-ticket.sh, the URL can't be built from the branch: the project
# is only known to the tracker, so this asks it (tracker.sh), with the key
# worktree-from-ticket uses. The binding runs it in the background, so the
# wait never holds up tmux.

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
  tmux display-popup -c "$client" -E -w 72 -h 9 -T ' Project ' \
    -e "LINEAR_PROJECT_MESSAGE=$1" "$(printf %q "$0") --show"
}

branch=$(git -C "$dir" branch --show-current 2>/dev/null)
[ -n "$branch" ] || { explain "This pane isn't on a git branch, so there's no ticket to find a project for."; exit 0; }

# Matches any team or project key, the same way linear-ticket.sh does.
ticket=$(printf '%s' "$branch" | grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
[ -n "$ticket" ] || { explain "The branch $branch doesn't name a ticket, so there's no project to open."; exit 0; }

tracker=$("${0%/*}/tracker.sh" name 2>/dev/null) || { explain "The ticket tracker is off in Settings."; exit 0; }
project=$([ "$tracker" = Jira ] && echo epic || echo project)
tmux display-message -c "$client" "Looking up the $tracker $project for $ticket..."

url=$("${0%/*}/tracker.sh" project "$ticket" 2>&1)
case $? in
  0) open "$url" ;;
  3) explain "$tracker has no ticket $ticket, so there's no $project to open." ;;
  4) explain "$ticket isn't in a $tracker $project." ;;
  *) explain "$tracker couldn't be asked about $ticket: ${url:-no answer}" ;;
esac
