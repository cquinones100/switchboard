#!/usr/bin/env bash

# Prints tmux-resurrect's save folder, found as resurrect finds it
# (scripts/helpers.sh): its @resurrect-dir, with ~, $HOME and $HOSTNAME
# expanded, else ~/.tmux/resurrect when that is there, else the XDG one.
# window-options.sh and resurrect-tidy.sh both use it, so neither works on a
# folder resurrect doesn't save to.

set -u

if [ -d "$HOME/.tmux/resurrect" ]; then
  dir="$HOME/.tmux/resurrect"
else
  dir="${XDG_DATA_HOME:-$HOME/.local/share}/tmux/resurrect"
fi
custom=$(tmux show -gqv @resurrect-dir 2>/dev/null)
[ -n "$custom" ] && dir=$(printf '%s' "$custom" | sed "s,\$HOME,$HOME,g; s,\$HOSTNAME,$(hostname),g; s,~,$HOME,g")
printf '%s\n' "$dir"
