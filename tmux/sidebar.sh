#!/usr/bin/env bash

# The tmux side of bin/tmux-sidebar: starting its daemon, and what the
# bindings in tmux.conf ask of it.
#
# Usage:
#   sidebar.sh start              start the daemon unless it is running, and lay
#                                 out the status line for the sidebar
#   sidebar.sh nudge              tell the daemon something changed
#   sidebar.sh focus <window>     M-s: put the cursor in the window's sidebar
#   sidebar.sh notices <client>   M-p: to the notifications, as the sidebar's n goes
#   sidebar.sh refocus <window>   the tab in front closed: the same, if the
#                                 sidebar is on
#   sidebar.sh mission <client> [work:tabs|work:untabbed]
#                                 M-Space: open mission control on that client;
#                                 M-j, M-o: on the Work map as a picker
#   sidebar.sh toggle             turn the sidebar on or off (@sidebar_on)
#
# The daemon is a node script, and the tmux server keeps the environment it
# was started with, where nvm (set up in ~/.zshrc) put no node: so node is
# found here from nvm's default alias, the cheap way, without loading nvm.

set -u

export LC_ALL=en_US.UTF-8

state_dir=~/.local/state/tmux-sidebar
client=~/.config/tmux/sidebar-pane.zsh

running() {
  local pid=""
  # The daemon writes its pid with no newline after it, so read says it
  # reached the end without one; what it read is all that counts.
  read -r pid _ 2>/dev/null <"$state_dir/daemon.pid"
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

nudge() {
  [ -d "$state_dir" ] && echo >>"$state_dir/nudge"
}

# With the sidebar on, the tabs are in it, so the status line drops its row of
# tabs and keeps one line: the session and its counts, then what the second
# line showed about the focused window. Off, the two lines tmux.conf sets. The
# first line's own format is kept in @sidebar_status_format while the sidebar
# is on and put back after: unsetting one line of status-format leaves it
# empty, not at tmux's default.
status_layout() {
  local saved
  saved=$(tmux show -gqv @sidebar_status_format)
  if [ "$(tmux show -gqv @sidebar_on)" = off ]; then
    tmux set -g status 2
    [ -n "$saved" ] && tmux set -g 'status-format[0]' "$saved" \; set -gu @sidebar_status_format
  else
    [ -n "$saved" ] || tmux set -g @sidebar_status_format "$(tmux show -gv 'status-format[0]')"
    # status-right is drawn too, though it shows nothing: tmux-continuum saves
    # the session from a #() job in it, which only runs while it is drawn.
    tmux set -g status on \; set -g 'status-format[0]' '#[align=left]#{T:status-left}#[default] #(~/.config/tmux/tab-status.sh #{q:pane_id} #{q:pane_current_path} #{q:pane_current_command} #{q:pane_tty} #{q:@sidebar})#[align=right]#{T:status-right}'
  fi
}

start() {
  status_layout
  running && return 0
  if ! command -v node >/dev/null 2>&1; then
    # nvm installs into ~/.nvm, or under XDG_CONFIG_HOME when that was set.
    for NVM_DIR in ${NVM_DIR:+"$NVM_DIR"} "$HOME/.nvm" "${XDG_CONFIG_HOME:-$HOME/.config}/nvm"; do
      [ -s "$NVM_DIR/nvm.sh" ] && break
    done
    default=$(cat "$NVM_DIR/alias/default" 2>/dev/null)
    candidate="$NVM_DIR/versions/node/v${default#v}/bin"
    if [ -n "$default" ] && [ -x "$candidate/node" ]; then
      export PATH="$candidate:$PATH"
    elif [ -s "$NVM_DIR/nvm.sh" ]; then
      . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
    fi
  fi
  mkdir -p "$state_dir" && chmod 700 "$state_dir"
  nohup ~/.local/bin/tmux-sidebar daemon </dev/null >>"$state_dir/daemon.log" 2>&1 &
  disown
}

case "${1:-}" in
  start) start ;;
  nudge) nudge ;;
  focus)
    window=${2:-}
    [ -n "$window" ] || exit 0
    if [ "$(tmux show -gqv @sidebar_on)" = off ]; then
      tmux set -gu @sidebar_on
      status_layout
    fi
    start
    nudge
    # A window that has no sidebar yet gets one from the daemon in a moment.
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      pane=$(tmux list-panes -t "$window" -F '#{pane_id} #{@sidebar}' 2>/dev/null | awk '$2 == "1" { print $1; exit }')
      [ -n "$pane" ] && { tmux select-pane -t "$pane"; exit 0; }
      sleep 0.1
    done
    "${0%/*}/say.sh" "The sidebar isn't running; see $state_dir/daemon.log."
    ;;
  notices)
    # M-p's "Go to the notifications": asked of the daemon for the client,
    # which goes there as the sidebar's n does, whatever a sidebar is doing
    # and with no sidebar in the window at all.
    # A daemon just started takes a moment to listen: tried again for up to
    # two seconds, then it says why.
    start
    for _ in $(seq 20); do
      printf 'go-notices %s\n' "${2:-}" | nc -U "$state_dir/sock" >/dev/null 2>&1 && exit 0
      sleep 0.1
    done
    "${0%/*}/say.sh" ${2:+-c "$2"} "The sidebar isn't running; see $state_dir/daemon.log."
    ;;
  refocus)
    # Unlike focus, never turns the sidebar on, and says nothing when there is
    # none: closing a tab isn't asking for one.
    window=${2:-}
    [ -n "$window" ] && [ "$(tmux show -gqv @sidebar_on)" != off ] && running || exit 0
    # The sidebar follows the tab in front, so it may still be on its way.
    for _ in 1 2 3 4 5 6 7 8 9 10; do
      pane=$(tmux list-panes -t "$window" -F '#{pane_id} #{@sidebar}' 2>/dev/null | awk '$2 == "1" { print $1; exit }')
      [ -n "$pane" ] && { tmux select-pane -t "$pane"; exit 0; }
      sleep 0.1
    done
    ;;
  mission)
    start
    tmux display-popup -c "${2:-}" -E -B -x 0 -y 0 -w 100% -h 100% "exec $(printf '%q ' "$client" mission "$state_dir/sock" "${2:-}" "${3:-}")"
    ;;
  toggle)
    if [ "$(tmux show -gqv @sidebar_on)" = off ]; then tmux set -gu @sidebar_on; else tmux set -g @sidebar_on off; fi
    start
    nudge
    ;;
esac
exit 0
