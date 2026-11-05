#!/bin/zsh

# Popup for the M-p palette's "Start work on a note's checklist items in new
# tabs (wtn)": asks which note, then which of its unticked items, and runs
# `wtn` for each item chosen in a tab of its own, the way wt-open.zsh runs
# `wt` for tickets.
#
# Usage: wtn-open.zsh
#        wtn-open.zsh items <note file>   prints the note's items, for the tests
#
# Both lists are fzf: the notes by title, most recently changed first (never
# their contents, nor deleted ones); the note's unticked top-level items in its
# order, leaving out any under a section named for parked work (## Defered,
# ## Later), each marked when its line says its plan still needs a choice. Tab
# picks several. The tabs start in the focused pane's directory (the
# binding's -d), so `wtn` finds that repository; each is named after its item
# and linked to the note (@tab_note), and the first is selected. `wtn` is typed
# into each tab's shell, which defines it from ~/.zshrc, with only the item's
# line number on the command line: the note's path goes in WTN_NOTE, set on the
# new tab with -e, never typed.

dim=$'\e[38;5;244m' red=$'\e[31m' yellow=$'\e[33m' reset=$'\e[0m'

# The note's unticked top-level items, one a line: its line number, a tab, the
# item's first line, and " (needs a choice)" when its line or the ones under
# it say so.
items() {
  awk -v mark=" (needs a choice)" '
    function flush() { if (n) printf "%d\t%s%s\n", n, text, (choice ? mark : ""); n = 0 }
    # A heading under a parked section is still in it; parked is the level of that section,
    # or 0. Parked words as lib/sidebar/todo.js has them.
    NR > 1 && /^#+[[:space:]]/ {
      flush(); match($0, /^#+/); level = RLENGTH
      if (!parked || level <= parked) parked = (tolower($0) ~ /(^|[^a-z])(later|defer+ed|parked|someday|backlog)([^a-z]|$)/) ? level : 0
      next
    }
    /^- \[ \] / && !parked { flush(); n = NR; text = substr($0, 7); sub(/[[:space:]]+$/, "", text); choice = ($0 ~ /needs (a )?choices?/); next }
    /^[^[:space:]]/ { flush(); next }
    n && /needs (a )?choices?/ { choice = 1 }
    END { flush() }
  ' "$1"
}

if [[ ${1:-} == items ]]; then
  items "$2"
  exit
fi

# extended_glob for the # in the pattern that takes the colors out of a pick.
setopt extended_glob

# Esc cancels; KEYTIMEOUT=1 keeps it from waiting for an escape sequence.
KEYTIMEOUT=1
fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

# Its lists look like mission control's boxes (fzf-style.sh).
export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${0:A:h}/fzf-style.sh" 2>/dev/null)"

git rev-parse --git-dir >/dev/null 2>&1 ||
  fail "This tab is not in a repository, so wtn has nowhere to make the worktrees. Run it from the repo the note's work belongs to."

notes=${NOTES_DIR:-$HOME/Notes}
[[ -d $notes ]] || fail "No notes directory at $notes."

# The notes, newest first: zsh's glob leaves out .trash and every other
# dot-folder, and .om orders by modification time.
files=($notes/**/*.md(N.om))
(( ${#files} )) || fail "No notes in $notes."
note=$(
  print -rl -- ${files#$notes/} | sed 's/\.md$//' |
    fzf --header='Enter: list the note'\''s items · Esc: cancel' --prompt='note> ' --layout=reverse --height=100%
) || exit 0
file=$notes/$note.md
[[ -f $file ]] || exit 0

list=$(items "$file")
[[ -n $list ]] || fail "$note has no unticked items."
picked=$(
  print -r -- "$list" | sed "s/ (needs a choice)\$/ ${yellow}(needs a choice)${reset}/" |
    fzf --ansi --multi --delimiter=$'\t' --with-nth=2 \
      --header='Enter: work on it in a new tab · Tab: pick several · Esc: cancel' \
      --prompt="${note:t}> " --layout=reverse --height=100%
) || exit 0

first=
for row in "${(@f)picked}"; do
  line=${row%%$'\t'*}
  [[ $line == <-> ]] || continue
  name=${${row#*$'\t'}//$'\e'\[[0-9;]#m/}
  name=${name%' (needs a choice)'}
  window=$(tmux new-window -d -c "$PWD" -e "WTN_NOTE=$note.md" -n "wtn" -P -F '#{window_id}') ||
    fail "tmux could not open a tab for: $name"
  tmux set-option -w -t "$window" @tab_name "${name[1,50]}"
  tmux set-option -w -t "$window" @tab_note "$note.md"
  tmux send-keys -t "$window" "wtn --item $line" Enter
  first=${first:-$window}
done
[[ -n $first ]] && tmux select-window -t "$first"
