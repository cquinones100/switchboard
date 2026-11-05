#!/usr/bin/env bash
# The tab for a note: goes to the tab linked to it (@tab_note, as /note-watch,
# /queue-and-clean and M-e's question set it), or, with none, asks which tab
# is the note's: one of the open tabs, linked to it then and gone to, or a new
# one through M-t's popup (new-tab.zsh), so you still pick its directory,
# linked to the note and named after it. Notes' t (sidebar.lua) runs it for the note
# under the cursor, and M-p's "Open the tab for the note in Notes" for the one
# Notes shows.
#
# Whichever tab it goes to, Notes moves beside the tab's Claude showing the
# note, as M-e puts it there (notes-open.sh side), the cursor left in Claude's
# pane; the note_tab_beside setting turns that off.
#
# Usage: note-tab.sh [<note>] [<client>]
#        note-tab.sh choose <note> <client>   the question, in its popup
#        note-tab.sh beside <window> <client> Notes beside <window>'s pane
#   <note>    a path in the notes directory; default, the one Notes shows
#             (.notes.json's last)
#   <client>  the tmux client to use; default, the one in front of this pane

set -u
# Its lists look like mission control's boxes (fzf-style.sh).
export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${BASH_SOURCE[0]%/*}/fzf-style.sh" 2>/dev/null)"
notes_dir=${NOTES_DIR:-$HOME/Notes}

# Notes beside the tab's Claude, showing the tab's note; already there, it is
# only shown it. Either way the cursor goes to Claude's pane.
if [ "${1:-}" = beside ]; then
  window=${2:-} client=${3:-}
  [ -n "$window" ] && [ "$(tmux show -gqv @setting_note_tab_beside)" != off ] || exit 0
  export LC_ALL=en_US.UTF-8
  . "${0%/*}/claude-pane.sh"
  # Claude's pane (claude-pane.sh), else the first that is neither the
  # sidebar nor Notes, as in a tab just opened, whose Claude is still starting.
  # Notes found first, as notes-open.sh finds it, a pane started by hand too,
  # so it is never taken for the tab's pane and the toggle in its side never
  # puts away Notes already in this tab.
  notes=$("${0%/*}/notes-open.sh" pane)
  [ -n "$notes" ] && [ "$(tmux display -p -t "$notes" '#{window_id}' 2>/dev/null)" = "$window" ] || notes=""
  pane="" first=""
  while IFS=$'\t' read -r id sidebar is_notes command tty; do
    { [ "$is_notes" = 1 ] || [ "$id" = "$notes" ]; } && continue
    [ "$sidebar" = 1 ] && continue
    [ -z "$first" ] && first=$id
    [ -z "$pane" ] && claude_pid "$command" "$tty" >/dev/null && pane=$id
  done < <(tmux list-panes -t "$window" -F $'#{pane_id}\t#{?@sidebar,1,0}\t#{?@notes_pane,1,0}\t#{pane_current_command}\t#{pane_tty}' 2>/dev/null)
  pane=${pane:-$first}
  [ -n "$pane" ] || exit 0
  if [ -n "$notes" ]; then
    "${0%/*}/notes-open.sh" show "$window"
  else
    "${0%/*}/notes-open.sh" side "$client" "$pane"
  fi
  tmux select-pane -t "$pane"
  exit 0
fi

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
    export NEW_TAB_NOTE=$note NEW_TAB_NAME=$title NEW_TAB_CLIENT=$client
    exec ~/.config/tmux/new-tab.zsh
  fi
  tmux set-option -w -t "$window" @tab_note "$note"
  if [ -n "$client" ]; then tmux switch-client -c "$client" -t "$window"; else tmux select-window -t "$window"; fi
  "$0" beside "$window" "$client"
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
  "$0" beside "$window" "$client"
  exit 0
fi

title=$(head -1 "$notes_dir/$note" | sed 's/^#* *//')
tmux display-popup ${client:+-c "$client"} -d "$HOME" -E -w 90 -h 60% -T "#[bold] A tab for ${title//#/##} " \
  "$(printf %q "$0") choose $(printf %q "$note") $(printf %q "$client")"
