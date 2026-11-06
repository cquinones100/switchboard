#!/usr/bin/env bash
# M-p's "How you use Switchboard": what the habits setting has counted, in a
# popup (lib/habits-report.js). A popup's PATH has no nvm, so it runs under an
# interactive zsh, as search.sh does; the script's path reaches it in the
# environment.
export HABITS_JS="${SWITCHBOARD_DIR:-$HOME/switchboard}/lib/habits-report.js"
exec zsh -i +m -c 'node "$HABITS_JS"'
