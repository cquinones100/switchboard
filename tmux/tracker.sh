#!/usr/bin/env bash
# Runs lib/tracker/cli.js, which asks the ticket tracker (Linear or Jira, as
# the tracker setting says) for the shell scripts here: see its comment for
# the commands. tmux runs scripts with the environment it was started with,
# where nvm (set up in ~/.zshrc) put no node, so node is found here as
# sidebar.sh finds it.
#
# Usage: tracker.sh <command> [ID]

set -u

repo=${SWITCHBOARD_DIR:-$HOME/switchboard}
# A clone not yet set up under its new name (setup.sh links it).
[ -d "$repo" ] || repo=$HOME/editor-configs

if ! command -v node >/dev/null 2>&1; then
  NVM_DIR="${NVM_DIR:-$HOME/.nvm}"
  default=$(cat "$NVM_DIR/alias/default" 2>/dev/null)
  candidate="$NVM_DIR/versions/node/v${default#v}/bin"
  if [ -n "$default" ] && [ -x "$candidate/node" ]; then
    export PATH="$candidate:$PATH"
  elif [ -s "$NVM_DIR/nvm.sh" ]; then
    . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
  fi
fi

exec node "$repo/lib/tracker/cli.js" "$@"
