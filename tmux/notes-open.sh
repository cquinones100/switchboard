#!/usr/bin/env bash
# Opens the notes app (bin/notes) in its own tab, or beside the pane you are
# in, or goes to it when it is already open: there is only ever one, since two
# Neovims saving the same notes as you type would write over each other. M-N
# runs it for a tab, M-e beside the pane.
#
# Usage: notes-open.sh <client_name>            M-N: its own tab, or go to it
#        notes-open.sh side <client_name> <pane> M-e: to the right of <pane>
#        notes-open.sh away <client_name> <pane> M-w on Notes: back to its own tab
#        notes-open.sh show <window>           Notes beside <window> shows its note
#        notes-open.sh pane                    prints the notes pane, if any
#        notes-open.sh note <client_name> <note>  shows <note> (a path in the
#                                              notes directory), then as M-N
#
# The notes pane is found by its @notes_pane option, which this sets, so it is
# found wherever it has been moved, and renaming its tab doesn't lose it; a
# tab of its own also carries the @notes window option. It runs notes from an
# interactive zsh, which has the PATH ~/.zshrc sets up (~/.local/bin, nvm),
# unlike the tmux server's own environment, and it closes when you quit notes.
#
# Beside a pane: notes already open elsewhere moves there (join-pane), its own
# tab closing behind it; open in that tab already, it goes back to a tab of its
# own (break-pane), so the key opens and puts away the same pane.

set -u

# The pane marked @notes_pane, else the first pane of a tab marked @notes, as
# a tab opened before panes were marked is, else the pane whose terminal runs
# the Neovim holding the notes lock (store.lua's .notes.lock), found up its
# parents, since that one has no terminal of its own: a pane tmux-resurrect
# brought back has lost its marks. A pane found from the lock is marked again.
notes_dir=${NOTES_DIR:-$HOME/Notes}
notes_pane() {
  local found pid tty hops=0
  # Every field is a 0 or 1, never empty: awk would close up an empty one and
  # read the next as it, taking a pane with only its tab's @notes for the
  # notes pane. Sidebar panes are never it, though they sit first in the tab.
  found=$(tmux list-panes -a -F '#{pane_id} #{?@notes_pane,1,0} #{?@notes,1,0} #{?@sidebar,1,0}' 2>/dev/null |
    awk '$4 == "1" { next } $2 == "1" { print $1; found = 1; exit } $3 == "1" && !old { old = $1 } END { if (!found && old) print old }')
  if [ -z "$found" ] && pid=$(cat "$notes_dir/.notes.lock" 2>/dev/null) && [ -n "$pid" ]; then
    while [ -n "$pid" ] && [ "$pid" != 1 ] && (( hops++ < 6 )); do
      tty=$(ps -o tty= -p "$pid" 2>/dev/null | tr -d ' ')
      [ -n "$tty" ] && [ "$tty" != "??" ] && break
      pid=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
    done
    [ -n "$tty" ] && [ "$tty" != "??" ] && found=$(tmux list-panes -a -F '#{pane_id} #{pane_tty}' | awk -v t="/dev/$tty" '$2 == t { print $1; exit }')
    [ -n "$found" ] && tmux set-option -p -t "$found" @notes_pane 1
  fi
  echo "$found"
}

# What runs notes; the tests put a stand-in here.
notes_command=${NOTES_OPEN_COMMAND:-zsh -i -c notes}

# Each tab's note: the one its Claude works from (@tab_note, which /note-watch
# and /queue-and-clean set once a note is settled), else the one Notes last
# showed beside it (@tab_note_last, kept here as Notes leaves it, but only once
# the tab has a note or another was opened while beside it: a tab with none
# that Notes merely passed through, still showing the last tab's note, would
# otherwise take that note as its own; the app itself records a note opened
# there as soon as it is, dropping the arrival marker, store.opened_beside,
# so quitting Notes keeps it and going back to the first counts too). Both are
# paths in the notes directory. M-e opens it in the running app, through the
# socket bin/notes listens on, or, for an app not yet running, by making it
# the note the app opens with (.notes.json's last).
notes_socket=${NOTES_SOCKET:-$HOME/.local/state/switchboard/notes.sock}
shown() { jq -r '.last // empty' "$notes_dir/.notes.json" 2>/dev/null; }
remember() {
  local window=$1 note arrived
  [ "$(tmux show -wqv -t "$window" @notes)" = 1 ] && return
  note=$(shown)
  arrived=$(tmux show -wqv -t "$window" @tab_note_arrived)
  tmux set-option -wu -t "$window" @tab_note_arrived 2>/dev/null
  [ -n "$note" ] || return 0
  [ -n "$(note_for "$window")" ] || [ "$note" != "$arrived" ] || return 0
  tmux set-option -w -t "$window" @tab_note_last "$note"
}
# What Notes showed as it came beside a tab, for remember to tell whether
# another note was opened there.
arrived() {
  local window=$1 note
  note=$(note_for "$window") || note=$(shown)
  [ -n "$note" ] && tmux set-option -w -t "$window" @tab_note_arrived "$note"
}
note_for() {
  local window=$1 want
  want=$(tmux show -wqv -t "$window" @tab_note)
  [ -n "$want" ] && [ -f "$notes_dir/$want" ] || want=$(tmux show -wqv -t "$window" @tab_note_last)
  [ -n "$want" ] && [ -f "$notes_dir/$want" ] && printf '%s' "$want"
}
show_for() {
  local window=$1 want
  want=$(note_for "$window") || return 0
  [ "$want" = "$(shown)" ] && return
  # A ' in the note's name is doubled, as a Vim string in ' wants it.
  local quoted=${want//\'/\'\'}
  nvim --server "$notes_socket" --remote-expr "luaeval('require(\"notes\").open(require(\"notes.store\").absolute(_A))', '$quoted')" >/dev/null 2>&1
}
# The app's sidebar is put away while Notes sits beside another tab's pane,
# where the note wants the room, and shown again in Notes' own tab; with the
# notes_hide_sidebar setting off, it is shown everywhere.
sidebar_shown() {
  local show=$1
  [ "$(tmux show -gqv @setting_notes_hide_sidebar)" = off ] && show=true
  nvim --server "$notes_socket" --remote-expr "luaeval('require(\"notes\").show_sidebar(_A)', v:$show)" >/dev/null 2>&1
}
open_with() {
  local window=$1 want meta
  want=$(note_for "$window") || return 0
  meta="$notes_dir/.notes.json"
  { [ -f "$meta" ] && jq --arg last "$want" '.last = $last' "$meta" || jq -n --arg last "$want" '{last: $last}'; } >"$meta.tmp" 2>/dev/null && mv "$meta.tmp" "$meta"
}

# A tab of its own for the notes pane, named Notes.
own_tab() {
  local window=$1 session
  tmux set-option -w -t "$window" @notes 1
  tmux set-option -w -t "$window" @tab_name "Notes"
  session=$(tmux display-message -p -t "$window" '#{session_name}')
  "${0%/*}/window-names.sh" "$session"
}

# Puts the notes pane back in a tab of its own, unless it is alone in its tab
# already (beside the sidebar). Returns whether it moved.
# The pane standing in for Notes in its tab while Notes is away
# (notes-away.sh), if there is one.
placeholder() {
  tmux list-panes -a -F '#{pane_id} #{@notes_placeholder}' 2>/dev/null | awk '$2 == "1" { print $1; exit }'
}

# Back in the Notes tab it left, in the place of what stood in for it there,
# or in a new Notes tab. Alone in its tab already, it stays, unless told it
# was left there (orphan, below).
put_away() {
  local pane=$1 left=${2:-} window stand_in
  [ -n "$left" ] || [ "$(tmux list-panes -t "$pane" -F '#{?@sidebar,,x}' | grep -c x)" -gt 1 ] || return 1
  stand_in=$(placeholder)
  if [ -n "$stand_in" ] && tmux join-pane -d -s "$pane" -t "$stand_in"; then
    tmux kill-pane -t "$stand_in"
  elif [ -n "$stand_in" ] && [ -z "$left" ]; then
    # A join that fails (a stand-in too small to split) leaves Notes, and its
    # sidebar, where they were; left alone, it takes a new tab instead.
    return 0
  else
    window=$(tmux break-pane -d -s "$pane" -P -F '#{window_id}') && own_tab "$window" || return 1
    [ -n "$stand_in" ] && tmux kill-pane -t "$stand_in"
  fi
  sidebar_shown true
  return 0
}

# A pane closed (the pane-exited and after-kill-pane hooks): Notes left alone
# in a tab that isn't its own, its work there gone, goes back to its own tab,
# so it never holds a tab but Notes'. The tab, now only a sidebar, closes.
# Only a tab M-e brought it beside (@notes_beside) counts: a Notes tab that
# lost its marks to tmux-resurrect looks the same otherwise, and stays.
if [ "${1:-}" = orphan ]; then
  pane=$(notes_pane)
  [ -n "$pane" ] || exit 0
  window=$(tmux display -p -t "$pane" '#{window_id}')
  [ "$(tmux show -wqv -t "$window" @notes)" = 1 ] && exit 0
  [ "$(tmux show -wqv -t "$window" @notes_beside)" = 1 ] || exit 0
  [ "$(tmux list-panes -t "$pane" -F '#{?@sidebar,,x}' | grep -c x)" -eq 1 ] || exit 0
  remember "$window"
  put_away "$pane" left
  exit 0
fi

# A tab's note just chosen (note-pick.sh): shown, if Notes is beside it.
if [ "${1:-}" = show ]; then
  window=${2:-}
  existing=$(notes_pane)
  [ -n "$window" ] && [ -n "$existing" ] && [ "$(tmux display -p -t "$existing" '#{window_id}')" = "$window" ] && show_for "$window"
  exit 0
fi

# A tab with no note of its own yet, neither one chosen nor one shown beside
# it before, is asked which it is (note-pick.sh), until it is told to keep
# whatever is open, with the ask_tab_note setting on.
ask_note() {
  local window=$1 client=$2
  [ "$(tmux show -gqv @setting_ask_tab_note)" = off ] && return
  [ -n "$(note_for "$window")" ] || [ -n "$(tmux show -wqv -t "$window" @tab_note_asked)" ] && return
  [ "$(tmux show -wqv -t "$window" @notes)" = 1 ] && return
  "${0%/*}/note-pick.sh" ask "$window" "$client"
}

# M-w on the notes pane: back to its own tab; alone in it already, asked
# about and closed as any pane is (confirm.sh, with @confirm_pane M-w set).
if [ "${1:-}" = away ]; then
  client=${2:-} pane=${3:-}
  [ -n "$pane" ] || exit 0
  remember "$(tmux display -p -t "$pane" '#{window_id}')"
  put_away "$pane" && exit 0
  tmux display-popup -c "$client" -E -w 60 -h 6 "~/.config/tmux/confirm.sh 'Close pane?'"
  exit 0
fi

# The notes pane, as found above, for note-tab.sh to tell whether Notes is in
# a tab already (even one started by hand, with no mark).
if [ "${1:-}" = pane ]; then
  notes_pane
  exit 0
fi

if [ "${1:-}" = side ]; then
  client=${2:-} here=${3:-}
  [ -n "$here" ] || exit 0
  existing=$(notes_pane)
  here_window=$(tmux display -p -t "$here" '#{window_id}')
  if [ -z "$existing" ]; then
    ask_note "$here_window" "$client"
    open_with "$here_window"
    arrived "$here_window"
    sidebar=on
    [ "$(tmux show -gqv @setting_notes_hide_sidebar)" = off ] || sidebar=off
    pane=$(tmux split-window -h -l 40% -t "$here" -c "$HOME" -e "NOTES_SIDEBAR=$sidebar" -P -F '#{pane_id}' "$notes_command") || exit 0
    tmux set-option -w -t "$here_window" @notes_beside 1
    tmux set-option -p -t "$pane" @notes_pane 1
    exit 0
  fi
  tmux set-option -p -t "$existing" @notes_pane 1
  if [ "$(tmux display -p -t "$existing" '#{window_id}')" = "$(tmux display -p -t "$here" '#{window_id}')" ]; then
    # Alone in its own tab (beside the sidebar) it is already put away.
    remember "$here_window"
    put_away "$existing"
    exit 0
  fi
  # Leaving its own Notes tab, it leaves notes-away.sh in its place, so the
  # tab stays and says where Notes went. Leaving another tab, nothing.
  old=$(tmux display -p -t "$existing" '#{window_id}')
  old_arrived=$(tmux show -wqv -t "$old" @tab_note_arrived)
  remember "$old"
  # Its own tab: marked @notes, or holding nothing else but the sidebar, as a
  # Notes tab that lost its marks (tmux-resurrect) does.
  alone=$(tmux list-panes -t "$existing" -F '#{?@sidebar,,x}' | grep -c x)
  if { [ "$(tmux show -wqv -t "$old" @notes)" = 1 ] || [ "$alone" -eq 1 ]; } && [ -z "$(placeholder)" ]; then
    stand_in=$(tmux split-window -d -t "$existing" -c "$HOME" -P -F '#{pane_id}' "${0%/*}/notes-away.sh")
    [ -n "$stand_in" ] && tmux set-option -p -t "$stand_in" @notes_placeholder 1 && own_tab "$old"
  fi
  ask_note "$here_window" "$client"
  # A move that fails (a pane too narrow to split) leaves Notes where it was,
  # its sidebar and the tab's arrival marker as they were, and takes back the
  # stand-in just left for it.
  if ! tmux join-pane -h -l 40% -s "$existing" -t "$here"; then
    [ -n "$old_arrived" ] && tmux set-option -w -t "$old" @tab_note_arrived "$old_arrived"
    stand_in=$(placeholder)
    [ -n "$stand_in" ] && [ "$(tmux display -p -t "$stand_in" '#{window_id}')" = "$old" ] && tmux kill-pane -t "$stand_in"
    exit 0
  fi
  # Brought into the Notes tab itself, it takes the stand-in's place there.
  stand_in=$(placeholder)
  if [ -n "$stand_in" ] && [ "$(tmux display -p -t "$stand_in" '#{window_id}')" = "$(tmux display -p -t "$here" '#{window_id}')" ]; then
    tmux kill-pane -t "$stand_in"
    sidebar_shown true
  else
    tmux set-option -w -t "$here_window" @notes_beside 1
    sidebar_shown false
  fi
  show_for "$here_window"
  arrived "$here_window"
  exit 0
fi

# A note to read (the sidebar's new-note notification): made the one Notes
# shows, in the running app or for the one about to start, then gone to as
# M-N goes.
if [ "${1:-}" = note ]; then
  want=${3:-}
  if [ -n "$want" ] && [[ $want != *..* ]] && [ -f "$notes_dir/$want" ]; then
    meta="$notes_dir/.notes.json"
    if [ -n "$(notes_pane)" ]; then
      quoted=${want//\'/\'\'}
      nvim --server "$notes_socket" --remote-expr "luaeval('require(\"notes\").open(require(\"notes.store\").absolute(_A))', '$quoted')" >/dev/null 2>&1
    else
      { [ -f "$meta" ] && jq --arg last "$want" '.last = $last' "$meta" || jq -n --arg last "$want" '{last: $last}'; } >"$meta.tmp" 2>/dev/null && mv "$meta.tmp" "$meta"
    fi
  fi
  set -- "${2:-}"
fi

client=${1:-}

existing=$(notes_pane)
if [ -n "$existing" ]; then
  window=$(tmux display -p -t "$existing" '#{window_id}')
  if [ -n "$client" ]; then
    tmux switch-client -c "$client" -t "$window"
  else
    tmux select-window -t "$window"
  fi
  tmux select-pane -t "$existing"
  exit 0
fi

window=$(tmux new-window -P -F '#{window_id}' -c "$HOME" "$notes_command")
tmux set-option -p -t "$window" @notes_pane 1
own_tab "$window"
