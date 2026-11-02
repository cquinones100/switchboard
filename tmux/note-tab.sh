#!/usr/bin/env bash
# The tab for a note: goes to the tab linked to it (@tab_note, as /note-watch,
# /queue-and-clean and M-e's question set it), or, with none, asks which tab
# is the note's: one of the open tabs, linked to it then and gone to, or a new
# one through M-t's popup (new-tab.zsh), so you still pick its directory,
# linked to the note and named after it. Notes' t (sidebar.lua) runs it for the note
# under the cursor, and M-p's "Open the tab for the note in Notes" for the one
# Notes shows.
#
# Usage: note-tab.sh [<note>] [<client>]
#        note-tab.sh choose <note> <client>   the question, in its popup
#   <note>    a path in the notes directory; default, the one Notes shows
#             (.notes.json's last)
#   <client>  the tmux client to use; default, the one in front of this pane

set -u
notes_dir=${NOTES_DIR:-$HOME/Notes}

if [ "${1:-}" = choose ]; then
  note=$2 client=${3:-}
  title=$(head -1 "$notes_dir/$note" | sed 's/^#* *//')
  # The open tabs, as the tab bar names them: no sidebar, no Notes, no
  # Notifications view, no window holding a parked pane. A line each:
  # "<window id>\t<number> <name>".
  tabs=$(tmux list-panes -a -F $'#{window_id}\t#{window_index}\t#{?@tab_label,#{@tab_label},#{window_name}}\t#{@sidebar}\t#{@pane_parked}\t#{@notes}\t#{@notices_window}' |
    awk -F'\t' '$5 != "" || $6 != "" || $7 != "" { skip[$1] = 1 } $4 != "" { next } !($1 in name) { order[++n] = $1; name[$1] = $2 " " $3 }
      END { for (i = 1; i <= n; i++) if (!(order[i] in skip)) print order[i] "\t" name[order[i]] }')
  choice=$({ printf 'new\t+ New tab, in a directory you pick\n'; printf '%s\n' "$tabs"; } |
    fzf --delimiter '\t' --with-nth 2 --layout reverse --prompt 'Tab: ' --header "Which tab is ${title}'s? Enter links it and goes there") || exit 0
  window=${choice%%$'\t'*}
  if [ "$window" = new ]; then
    export NEW_TAB_NOTE=$note NEW_TAB_NAME=$title
    exec ~/.config/tmux/new-tab.zsh
  fi
  tmux set-option -w -t "$window" @tab_note "$note"
  if [ -n "$client" ]; then tmux switch-client -c "$client" -t "$window"; else tmux select-window -t "$window"; fi
  exit 0
fi
note=${1:-$(jq -r '.last // empty' "$notes_dir/.notes.json" 2>/dev/null)}
client=${2:-$(tmux display -p ${TMUX_PANE:+-t "$TMUX_PANE"} '#{client_name}' 2>/dev/null)}

if [ -z "$note" ] || [[ $note == *..* ]] || [ ! -f "$notes_dir/$note" ]; then
  "${0%/*}/say.sh" ${client:+-c "$client"} "No note to open a tab for"
  exit 0
fi

# Its tab, if one is linked to it; not Notes' own.
window=$(tmux list-windows -a -F $'#{window_id}\t#{@tab_note}\t#{@notes}' | awk -F'\t' -v n="$note" '$2 == n && $3 != "1" { print $1; exit }')
if [ -n "$window" ]; then
  if [ -n "$client" ]; then
    tmux switch-client -c "$client" -t "$window"
  else
    tmux select-window -t "$window"
  fi
  exit 0
fi

title=$(head -1 "$notes_dir/$note" | sed 's/^#* *//')
tmux display-popup ${client:+-c "$client"} -d "$HOME" -E -w 90 -h 60% -T " A tab for ${title//#/##} " \
  "$(printf %q "$0") choose $(printf %q "$note") $(printf %q "$client")"
