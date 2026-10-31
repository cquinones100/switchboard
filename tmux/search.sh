#!/usr/bin/env bash
# Search everything (M-S-f): asks for a query, then lists what your notes,
# your ticket tracker, GitHub and Slack hold for it (lib/search.js), each source's results
# arriving as it answers, Slack's last. fzf narrows them as you type; Enter
# reads one beside the pane, in the link view (tmux/link-view.sh), a note as
# its Markdown; Ctrl-O opens a link in the browser instead.
#
# Usage: search.sh <pane>
#
# It runs in a popup, whose PATH has no nvm, so the search runs under an
# interactive zsh, as the link view's pane does.

set -u
pane=${1:-}
here=${0%/*}
repo=${SWITCHBOARD_DIR:-$HOME/switchboard}
[ -n "$pane" ] || exit 0

printf '\n'
read -r -e -p '  Search notes, tickets, GitHub and Slack: ' query || exit 0
[ -n "${query// /}" ] || exit 0

choice=$(zsh -i -c 'node "$1" "$2"' _ "$repo/lib/search.js" "$query" 2>/dev/null |
  fzf --ansi --delimiter '\t' --with-nth 1 --prompt 'Narrow: ' --layout reverse --expect ctrl-o \
    --header "\"$query\" · Slack's results come last, in a few seconds · Enter reads it beside the pane · Ctrl-O opens it in the browser") || exit 0
key=$(printf '%s' "$choice" | sed -n 1p)
url=$(printf '%s' "$choice" | sed -n 2p | cut -f2)
[ -n "$url" ] || exit 0
if [ "$key" = ctrl-o ] && [[ $url =~ ^https?:// ]]; then exec "$here/open-url.sh" "$url"; fi
exec "$here/link-view.sh" open "$pane" "$url"
