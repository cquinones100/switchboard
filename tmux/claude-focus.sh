#!/usr/bin/env bash
# M-c: puts the cursor in the pane running Claude in the tab in front, as
# claude-pane.sh tells one (a foreground process with a session file), the
# sidebar and any other pane passed over. With none, it says so.
#
# Usage: claude-focus.sh <window>

set -u
# tmux writes the tabs between fields as they are only under a UTF-8 locale,
# which the server's environment may not have.
export LC_ALL=en_US.UTF-8
window=${1:-}
[ -n "$window" ] || exit 0
. "${0%/*}/claude-pane.sh"

# Every field is filled (@sidebar as 1 or 0): read splits on tabs as
# whitespace, so an empty one would shift the rest.
while IFS=$'\t' read -r pane sidebar command tty; do
  [ "$sidebar" = 1 ] && continue
  if claude_pid "$command" "$tty" >/dev/null; then
    tmux select-pane -t "$pane"
    exit 0
  fi
done < <(tmux list-panes -t "$window" -F $'#{pane_id}\t#{?@sidebar,1,0}\t#{pane_current_command}\t#{pane_tty}')
"${0%/*}/say.sh" "No Claude in this tab"
