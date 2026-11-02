#!/usr/bin/env bash

# Runs slack-relay from a tmux binding: `start` (the default, M-S) opens the
# relay's tab or goes to it if it is already running, `stop` (C-M-s) stops it.
#
# Usage: slack-relay-open.sh [start|stop]
#
# The tmux server keeps the environment it was started with, which has no
# node (nvm is set up in ~/.zshrc, which only interactive shells read), so a
# node script run straight from a binding dies on `env: node: No such file or
# directory`. The default Node is put on the PATH first, as sidebar.sh does.
# The relay's own tab runs an interactive zsh, so it has node either way.

set -u

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

action=${1:-start}

# A binding has nowhere to show output, so it goes in a box (say.sh): errors
# always, and for stop the confirmation too.
if ! output=$(~/.local/bin/slack-relay "$action" 2>&1); then
  "${0%/*}/say.sh" "slack-relay: ${output:-could not $action}"
elif [ "$action" = stop ]; then
  "${0%/*}/say.sh" "slack-relay: ${output:-stopping}"
fi
