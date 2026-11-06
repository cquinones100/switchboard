#!/usr/bin/env bash
# Which tmux server Switchboard runs on, for the scripts that can run outside
# it: Claude Code's hooks (agent-state.sh) run for every Claude session, in any
# tmux server or none, and setup.sh and the "My tabs" command in
# claude/switchboard.md run from any terminal.
#
# The tmux_server setting says: "default", tmux's own server, which every tmux
# you start shares, or "switchboard", a server of its own (tmux -L
# switchboard), where tmux.conf loads Switchboard (tmux/own-server.conf) and
# every other tmux server stays plain tmux. Read with shell builtins alone,
# since agent-state.sh runs before every tool call of every Claude session.
#
# Sourced, it gives:
#   sb_server     sets SB_SERVER to the server's name (default or switchboard)
#   sb_tmux       runs tmux on Switchboard's server: its own by name, else the
#                 one tmux finds itself ($TMUX, or the default)
#   sb_tmux_env   prints what $TMUX is to be for a script that runs a bare tmux
#                 (sidebar.sh, continuum-save.sh) to reach Switchboard's
#                 server: its own server's socket, or nothing while that isn't
#                 running; else $TMUX as it is
#   sb_elsewhere  whether $TMUX is a tmux server other than Switchboard's own,
#                 one that stays plain tmux (never, on tmux's default server)
#
# Run with arguments, it runs tmux with them on Switchboard's server:
#   switchboard-tmux.sh list-panes -a -F '...'
#   switchboard-tmux.sh --name   prints the server's name (own-server.conf)

sb_server() {
  local file=${SWITCHBOARD_SETTINGS:-$HOME/.config/switchboard/settings.json} text="" re='"tmux_server"[[:space:]]*:[[:space:]]*"([^"]*)"'
  SB_SERVER=default
  [ -r "$file" ] || return 0
  IFS= read -r -d "" text <"$file" || :
  # Only a name the setting offers, as settings.sh takes any other for the
  # default.
  [[ $text =~ $re && ${BASH_REMATCH[1]} == switchboard ]] && SB_SERVER=${BASH_REMATCH[1]}
  return 0
}

sb_tmux() {
  sb_server
  if [ "$SB_SERVER" = default ]; then
    command tmux "$@"
  else
    command tmux -L "$SB_SERVER" "$@"
  fi
}

sb_tmux_env() {
  sb_server
  if [ "$SB_SERVER" = default ]; then
    printf '%s' "${TMUX:-}"
  else
    sb_tmux display -p '#{socket_path}' 2>/dev/null
  fi
}

sb_elsewhere() {
  sb_server
  [ "$SB_SERVER" != default ] && [ -n "${TMUX:-}" ] || return 1
  local socket=${TMUX%%,*}
  [ "${socket##*/}" != "$SB_SERVER" ]
}

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  if [ "${1:-}" = --name ]; then
    sb_server
    echo "$SB_SERVER"
    exit 0
  fi
  sb_tmux "$@"
  exit
fi
