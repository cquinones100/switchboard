#!/usr/bin/env bash

# Prints the label for the window <target> whose pane is at <path>: the name
# set with M-p's "Rename the tab", else the ticket and its title, else the name
# accent-color.sh gives the directory. path-facts.sh works out the last two.
#
# Usage: tab-label.sh <target> <path> [<label>]
#
# <label> is path-facts.sh's label for <path>, from a caller that has already
# asked it for the color and PR, so it isn't asked twice.
#
# window-names.sh puts this in the tab and tab-status.sh at the front of the
# second status line, so the two can never disagree about what a window is.
# Runs for every window on every pane focus, so it never waits on the network:
# path-facts.sh answers from caches.

set -u

target="${1:-}"
path="${2:-}"
label="${3:-}"
[ -n "$target" ] || exit 0

name=$(tmux show -wqv -t "$target" @tab_name 2>/dev/null)
if [ -n "$name" ]; then
  printf '%s' "$name"
  exit 0
fi

[ -n "$label" ] || IFS=$'\t' read -r _ label _ <<<"$("${0%/*}/path-facts.sh" "$path")"
printf '%s' "$label"
