#!/usr/bin/env bash

# Called by tmux pane-focus-in hook. Receives the current pane path and
# session ID as arguments, then updates accent colors for the active pane
# border, session badge, and current window tab from the focused pane, and
# hands the per-window tabs to window-names.sh.

pane_path="$1"
session="$2"

script_dir="$(dirname "$0")"

mode=$(cat "$script_dir/.theme-mode" 2>/dev/null || echo dark)
case "$mode" in
  light) BG_DARK=colour253; BG_MED=colour248; FG_BRIGHT=colour232; FG_DIM=colour238 ;;
  *)     BG_DARK=colour235; BG_MED=colour238; FG_BRIGHT=colour255; FG_DIM=colour244 ;;
esac

# From path-facts.sh, which caches it, so window-names.sh's ask for the same
# directory below is answered from the cache.
IFS=$'\t' read -r hex _ <<<"$("$script_dir/path-facts.sh" "$pane_path")"
tmux set -g pane-active-border-style "fg=#${hex}"
tmux set -g status-left "#[bg=#${hex},fg=$FG_BRIGHT,bold]  #S #{E:@agent_counts}#[bg=$BG_DARK] #{E:@palette_button}#(~/.config/tmux/health.sh)"
tmux setw -g window-status-current-format "#[bg=$BG_MED,fg=#${hex},bold] #I:#{E:@agent_marker}#[bg=$BG_MED,fg=#${hex},bold]#{?@agent, ,}#W "

"$script_dir/window-names.sh" "$session"
