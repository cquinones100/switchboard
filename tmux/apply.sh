#!/usr/bin/env bash

# apply.sh <action>: puts a change to Switchboard's code into effect in the
# tmux it runs in, for what keeps running the code it started with.
#
#   sidebar  restart the sidebar daemon (it is started again at once)
#   reload   reload tmux.conf
#   relay    restart the Slack relay, when it runs
#   panes    restart every sidebar pane and the Notifications window's client
#            with its own command, for a new sidebar-pane.zsh (mission
#            control's popup takes it when next opened)
#
# repo-update.sh runs sidebar and reload after an update, and queue-and-clean
# runs the ones bin/apply-actions lists after a landing, once you say yes.
# Plain tmux calls, so the server is the one $TMUX names.

set -u

case "${1:-}" in
  sidebar)
    pidfile=~/.local/state/tmux-sidebar/daemon.pid
    pid=$(cut -d' ' -f1 "$pidfile" 2>/dev/null)
    # Only the daemon itself, as setup.sh's restart_stale_daemon checks: a pid
    # left by a daemon that died may name some other process by now.
    command=$(ps -p "${pid:-0}" -o command= 2>/dev/null)
    if [[ $command =~ ^[^\ ]*node\ [^\ ]*/tmux-sidebar\ daemon$ ]] && kill "$pid" 2>/dev/null; then
      # Gone before the new one starts, or sidebar.sh start finds it alive.
      for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
    fi
    ~/.config/tmux/sidebar.sh start
    ;;
  reload)
    tmux source-file ~/.config/tmux/tmux.conf
    ;;
  relay)
    lock=~/.local/state/slack-relay/relay.lock
    pid=$(cut -d' ' -f1 "$lock" 2>/dev/null)
    [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null || { echo "the Slack relay isn't running" >&2; exit 1; }
    slack-relay stop >/dev/null || exit 1
    # Its shutdown may wait on Slack for up to 15 seconds, and start while it
    # still runs would only go to its tab.
    for _ in $(seq 1 100); do kill -0 "$pid" 2>/dev/null || break; sleep 0.2; done
    kill -0 "$pid" 2>/dev/null && { echo "the Slack relay didn't stop within 20 seconds" >&2; exit 1; }
    slack-relay start
    ;;
  panes)
    tmux list-panes -a -F '#{pane_id} #{@sidebar} #{@notices_window}' | while read -r pane sidebar notices; do
      [ "$sidebar" = 1 ] || [ "$notices" = 1 ] && tmux respawn-pane -k -t "$pane"
    done
    exit 0
    ;;
  *)
    echo "usage: apply.sh sidebar|reload|relay|panes" >&2
    exit 2
    ;;
esac
