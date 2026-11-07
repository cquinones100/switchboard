#!/usr/bin/env bash

# Carries the window options tmux-resurrect doesn't save across a restart:
# a tab's reminder (@remind_at, @remind_note, @remind_said, from remind.sh)
# and its note (@tab_note, @tab_note_last, @tab_note_asked, which M-e shows
# beside it, and @tab_note_arrived, the note Notes came beside it with,
# notes-open.sh), and whether the mode working a note opened it (@autopilot,
# @autopilot_in, @autopilot_item, @autopilot_left, @autopilot_seen,
# lib/sidebar/autopilot.js). Resurrect saves panes and layouts only, so
# without this every reminder and every tab's note was gone once the Mac
# restarted.
#
# `save`, after every resurrect save (@resurrect-hook-post-save-all in
# tmux.conf), writes each window holding one of OPTIONS to window-options.tsv
# in resurrect's directory, under a first line naming the save `last` points
# at: a line per window, its session, index, then each option's value.
# `restore`, after resurrect restores (@resurrect-hook-post-restore-all), sets
# them again on the window at the same session and index, which resurrect
# keeps, but only when that file was written for the save just restored.
#
# Usage: window-options.sh save | restore

set -u
export LC_ALL=en_US.UTF-8

OPTIONS=(@remind_at @remind_note @remind_said @tab_note @tab_note_last @tab_note_asked @tab_note_arrived @autopilot @autopilot_in @autopilot_item @autopilot_left @autopilot_seen)

# Resurrect's own folder, found as it finds it.
dir=$("${0%/*}/resurrect-dir.sh")
file="$dir/window-options.tsv"
[ -d "$dir" ] || exit 0
last=$(readlink "$dir/last" 2>/dev/null)
[ -n "$last" ] || exit 0

case ${1:-} in
  save)
    format='#{session_name}'$'\t''#{window_index}'
    for option in "${OPTIONS[@]}"; do format+=$'\t''#{'"$option"'}'; done
    # Owner-only, as tidy keeps the rest of the directory: a note is yours.
    umask 077
    {
      printf '%s\n' "$last"
      # Only windows holding at least one of the options: the columns after
      # the session and index aren't all empty.
      tmux list-windows -a -F "$format" 2>/dev/null | awk -F '\t' '{ for (i = 3; i <= NF; i++) if ($i != "") { print; next } }'
    } >"$file.$$" && mv "$file.$$" "$file"
    ;;
  restore)
    [ -f "$file" ] || exit 0
    { IFS= read -r saved_for && [ "$saved_for" = "$last" ]; } <"$file" || exit 0
    # Tabs made unit separators first, since read takes a tab for blank space
    # and runs empty columns together.
    tail -n +2 "$file" | tr '\t' '\037' | while IFS=$'\037' read -r -a fields; do
      target="=${fields[0]}:${fields[1]}"
      tmux display -p -t "$target" '' >/dev/null 2>&1 || continue
      for i in "${!OPTIONS[@]}"; do
        value=${fields[$((i + 2))]:-}
        # One saved empty is unset, so a tab that has options already, from a
        # restore into a running tmux, ends as it was saved.
        if [ -n "$value" ]; then
          tmux set -w -t "$target" "${OPTIONS[$i]}" "$value"
        else
          tmux set -wu -t "$target" "${OPTIONS[$i]}"
        fi
      done
    done
    ;;
  *)
    echo "Usage: window-options.sh save | restore" >&2
    exit 1
    ;;
esac
exit 0
