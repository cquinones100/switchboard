#!/usr/bin/env bash
# One Claude to a tab: a Claude session that starts in a tab already running
# one moves to a tab of its own, since the sidebar, mission control, the Slack
# relay, M-c and M-m each take a tab's Claude to be one pane. Claude Code's
# SessionStart hook can't refuse a session, so agent-state.sh runs this as one
# starts, in the background, with the one_claude_per_tab setting on.
#
# Usage: one-claude.sh <pane>   the pane the session started in

set -u
# tmux writes the tabs between fields as they are only under a UTF-8 locale,
# which the server's environment may not have.
export LC_ALL=en_US.UTF-8
pane=${1:-}
[ -n "$pane" ] || exit 0
window=$(tmux display -p -t "$pane" '#{window_id}' 2>/dev/null) || exit 0
. "${0%/*}/claude-pane.sh"

# Every field is filled (@sidebar as 1 or 0): read splits on tabs as
# whitespace, so an empty one would shift the rest.
while IFS=$'\t' read -r other sidebar command tty; do
  [ "$other" = "$pane" ] || [ "$sidebar" = 1 ] && continue
  if claude_pid "$command" "$tty" >/dev/null; then
    session=$(tmux display -p -t "$pane" '#{session_name}')
    tmux break-pane -s "$pane" || exit 0
    "${0%/*}/window-names.sh" "$session"
    # Said on a client showing that session, the one used last; with none,
    # to no one.
    client=$(tmux list-clients -t "$session" -F '#{client_activity} #{client_name}' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)
    [ -n "$client" ] && "${0%/*}/say.sh" -c "$client" "That tab already had a Claude, so this session has a tab of its own. (Settings: One Claude to a tab)"
    exit 0
  fi
done < <(tmux list-panes -t "$window" -F $'#{pane_id}\t#{?@sidebar,1,0}\t#{pane_current_command}\t#{pane_tty}')
