#!/usr/bin/env bash
# Search everything (M-S-f): asks for a query, then lists what your notes,
# Linear, GitHub and Slack hold for it (lib/search.js), each source's results
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

# The query is asked in fzf too, with nothing to pick, so Esc closes the
# popup there as it does everywhere else (readline's prompt took Esc as Meta).
# Enter with no match leaves fzf with status 1 and the query printed; Esc or
# Ctrl-C with 130.
query=$(fzf --print-query --no-info --layout reverse --height 100% --prompt '  Search notes, Linear, GitHub and Slack: ' </dev/null)
[ $? -eq 130 ] && exit 0
query=$(printf '%s' "$query" | sed -n 1p)
[ -n "${query// /}" ] || exit 0

# The search runs as fzf's own reload, so Esc ends it with fzf rather than
# leaving the popup open until Slack has answered. The query and script reach
# it in the environment, never in the command fzf runs.
export SEARCH_JS="$repo/lib/search.js" SEARCH_QUERY="$query"
choice=$(: | fzf --ansi --delimiter '\t' --with-nth 1 --prompt 'Narrow: ' --layout reverse --expect ctrl-o \
    --bind 'start:reload:zsh -i +m -c "node \"\$SEARCH_JS\" \"\$SEARCH_QUERY\" 2>/dev/null"' \
    --header "\"$query\" · Slack's results come last, in a few seconds · Enter reads it beside the pane · Ctrl-O opens it in the browser") || exit 0
key=$(printf '%s' "$choice" | sed -n 1p)
url=$(printf '%s' "$choice" | sed -n 2p | cut -f2)
[ -n "$url" ] || exit 0
if [ "$key" = ctrl-o ] && [[ $url =~ ^https?:// ]]; then exec "$here/open-url.sh" "$url"; fi
exec "$here/link-view.sh" open "$pane" "$url"
