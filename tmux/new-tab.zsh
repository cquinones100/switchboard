#!/bin/zsh

# Popup for M-t (and the sidebar's n): asks which directory the new tab opens
# in, then opens it there running Claude and asks for its name (tab-name.zsh).
#
# Usage: new-tab.zsh
#
# For a note (note-tab.sh sets NEW_TAB_NOTE, a path in the notes directory, and
# NEW_TAB_NAME, its title): the tab is linked to the note (@tab_note) and named
# after it, rather than asked for a name, and Notes moves beside its Claude
# showing the note (note-tab.sh beside, NEW_TAB_CLIENT the client). NEW_TAB_NAME alone names it, and
# NEW_TAB_PROMPT is Claude's first prompt (mission control's "Open a tab for
# this thread with Claude" on a Slack message), passed in the tab's
# environment, so nothing in it is read by a shell. A note's tab also starts
# Claude knowing which note it is for (--append-system-prompt, so no turn is
# spent on it): its title and file, the note itself read only when asked
# (note_tab_prompt setting).
#
# The directory comes from pick-dir.zsh, which shape-open.zsh asks with too.

dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'
script_dir=${0:A:h}

fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

dir=$("$script_dir/pick-dir.zsh" "A new tab running Claude: Enter opens it there") || exit 0

context=
if [[ -n ${NEW_TAB_NOTE:-} && $(tmux show -gqv @setting_note_tab_prompt) != off ]]; then
  notes_dir=${NOTES_DIR:-$HOME/Notes}
  context="This tab is for the note \"${NEW_TAB_NAME:-${NEW_TAB_NOTE:t:r}}\" in the user's notes app, the file ${${notes_dir/#$HOME/~}%/}/$NEW_TAB_NOTE, which Notes shows beside you. It is the user's own and private: read it only when they ask you to. Renamed or moved since, its file is the one the tab's @tab_note option names now, relative to the notes directory (tmux show -wqv -t \$TMUX_PANE @tab_note)."
fi
# tmux expands formats in -c, so a # in the path is doubled. The prompt and
# the note's context reach Claude through the tab's environment.
window=$(tmux new-window -P -F '#{window_id}' -c "${dir//\#/##}" -e "NEW_TAB_PROMPT=${NEW_TAB_PROMPT:-}" -e "NEW_TAB_CONTEXT=$context" \
  "zsh -i -c 'claude \${NEW_TAB_CONTEXT:+--append-system-prompt} \${NEW_TAB_CONTEXT:+\"\$NEW_TAB_CONTEXT\"} \${NEW_TAB_PROMPT:+\"\$NEW_TAB_PROMPT\"}; exec zsh -i'") || fail "tmux could not open a tab."
if [[ -n ${NEW_TAB_NOTE:-} || -n ${NEW_TAB_NAME:-} ]]; then
  [[ -n ${NEW_TAB_NOTE:-} ]] && tmux set-option -w -t "$window" @tab_note "$NEW_TAB_NOTE"
  tmux set-option -w -t "$window" @tab_name "${NEW_TAB_NAME:-${NEW_TAB_NOTE:r}}"
  "$script_dir/window-names.sh" "$(tmux display -p -t "$window" '#{session_name}')"
  # A note's tab opens with Notes beside its Claude, showing the note.
  [[ -n ${NEW_TAB_NOTE:-} ]] && "$script_dir/note-tab.sh" beside "$window" "${NEW_TAB_CLIENT:-}"
  exit 0
fi
cd "$dir" && exec "$script_dir/tab-name.zsh" optional
