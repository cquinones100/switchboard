#!/usr/bin/env bash
# M-y: copies the latest reply of the Claude running in the tab in front to the
# clipboard, read from its session transcript (lib/claude-reply.js), and says
# so. The Claude pane is found as claude-focus.sh finds it, the sidebar and any
# other pane passed over.
#
# Usage: reply-copy.sh <window>

set -u
# tmux writes the tabs between fields as they are only under a UTF-8 locale,
# which the server's environment may not have.
export LC_ALL=en_US.UTF-8
window=${1:-}
[ -n "$window" ] || exit 0
. "${0%/*}/claude-pane.sh"
say() { "${0%/*}/say.sh" "$1"; }

# Every field is filled (@sidebar as 1 or 0): read splits on tabs as
# whitespace, so an empty one would shift the rest. A tool standing in Claude's
# place (M-d, lazygit: pane-in-place.sh) names the pane it parked in
# @parked_pane, which is read in its stead.
fields=$'#{pane_id}\t#{?@sidebar,1,0}\t#{pane_current_command}\t#{pane_tty}\t#{?@agent_state,#{@agent_state},none}'
claude=
while IFS=$'\t' read -r pane sidebar command tty state parked; do
  [ "$sidebar" = 1 ] && continue
  if [ "$parked" != none ]; then
    IFS=$'\t' read -r pane sidebar command tty state < <(tmux display -p -t "$parked" "$fields")
  fi
  if claude_pid "$command" "$tty" >/dev/null; then
    claude=$pane
    break
  fi
done < <(tmux list-panes -t "$window" -F "$fields"$'\t#{?@parked_pane,#{@parked_pane},none}')
[ -n "$claude" ] || { say "No Claude in this tab"; exit 0; }

# A binding's shell has no nvm, so node runs under an interactive zsh, as
# search.sh runs it; the script and pane reach it in the environment.
export REPLY_JS="${SWITCHBOARD_DIR:-$HOME/switchboard}/lib/claude-reply.js" REPLY_PANE=$claude
reply=$(zsh -i +m -c 'node "$REPLY_JS" "$REPLY_PANE"' 2>/dev/null)
case $? in
  0) ;;
  4) say "Claude hasn't replied yet in this tab"; exit 0 ;;
  *) say "Couldn't find this tab's Claude session"; exit 0 ;;
esac

printf '%s' "$reply" | pbcopy
lines=$(printf '%s\n' "$reply" | wc -l | tr -d ' ')
words="Copied Claude's latest reply ($lines line$([ "$lines" = 1 ] || echo s))"
[ "$state" = working ] && words="$words, written while it is still working"
say "$words"
