#!/bin/zsh

# Popup for the M-p palette's "Start work on a note's checklist items in new
# tabs (wtn)": asks which note, then which of its unticked items, and runs
# `wtn` for each item chosen in a tab of its own, the way wt-open.zsh runs
# `wt` for tickets.
#
# Usage: wtn-open.zsh
#        wtn-open.zsh items <note file>         prints the note's items, for the tests
#        wtn-open.zsh item <note file> <line>   prints one item whole, for the preview
#        wtn-open.zsh queue <note> [--ready-only]          prints the items the mode would take, in order
#        wtn-open.zsh next <note> <count> [--ready-only]   opens tabs for the first <count> of them
#
# queue and next are the sidebar daemon's, for working a note continuously
# (lib/sidebar/autopilot.js), run in the note's repository: <note> is its path
# within the notes directory. The order is the items whose plan is settled
# (a "Plan: [[…]]" line, no "needs a choice" on it and no "## Decision needed"
# in the plan note, plan_of) first, then those with no plan, or a plan note
# that isn't there, each in the note's order; an item whose plan needs a choice is never
# taken, nor one under a parked section, nor one a worktree has or wtn started
# in the last 30 minutes (its claims file), nor one whose
# first line is a line of WTN_SKIP (the items skipped, and those the daemon
# opened a moment ago, before wtn has made their branch). --ready-only takes
# only the first kind. Each line of queue is the item's line number, a tab
# and its first line; next opens each tab as a pick below does, in the
# background, marks it @autopilot with the time it opened, and prints its line
# number, window and first line, tab-separated, and keeps the first line as
# the tab's @autopilot_item, which the daemon skips while the tab is open, so a
# tab whose wtn failed before making its branch doesn't have its item taken again. WTN_SESSION names the tmux
# session to open them in.
#
# Both lists are fzf: the notes by title, most recently changed first (never
# their contents, nor deleted ones); the note's unticked top-level items in its
# order, leaving out any under a section named for parked work (## Defered,
# ## Later) and any a worktree of this repository is already working on, each
# marked when its line says its plan still needs a choice, and the one under
# the cursor shown whole in a pane below the list, its full text and what is
# indented under it, since the list cuts it to one line (wtn_item_preview
# setting). Tab picks several. It asks first which repository (pick-dir.zsh), the focused
# pane's (the binding's -d) at the top, and the tabs start there, so `wtn`
# finds it; each is named after its item
# and linked to the note (@tab_note), and the first is selected. `wtn` is typed
# into each tab's shell, which defines it from ~/.zshrc, with only the item's
# line number on the command line: the note's path goes in WTN_NOTE, set on the
# new tab with -e, never typed.

zmodload zsh/datetime

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
    /^- \[ \] / && !parked { flush(); n = NR; text = substr($0, 7); sub(/^[[:space:]]+/, "", text); sub(/[[:space:]]+$/, "", text); choice = ($0 ~ /needs (a )?choices?/); next }
    /^[^[:space:]]/ { flush(); next }
    n && /needs (a )?choices?/ { choice = 1 }
    END { flush() }
  ' "$1"
}

# One item whole: its line and what is indented under it, blank lines between
# included, up to the next line of the note's top level, as wtn hands it to
# Claude (bin/worktree-from-note).
item() {
  awk -v at="$2" '
    NR == at { print; shown = 1; next }
    shown && /^[^[:space:]]/ { exit }
    shown && /^[[:space:]]*$/ { blank = blank "\n"; next }
    shown { printf "%s", blank; blank = ""; print }
  ' "$1"
}

# Those of them no worktree is working on yet. wtn keeps the item a branch is
# for in its git config, branch.<branch>.wtnItem, as "<note>: <item's first
# line>", the note's path within the notes directory; an item counts as taken
# only while its branch is checked out in a worktree, so one whose worktree
# was removed, its branch kept, shows again.
unclaimed() {
  local file=$1 note=${1#${NOTES_DIR:-$HOME/Notes}/} key value row text
  local -A claimed checked_out
  git worktree list --porcelain 2>/dev/null | while read -r key value; do
    [[ $key == branch ]] && checked_out[${value#refs/heads/}]=1
  done
  git config --get-regexp '^branch\..*\.wtnitem$' 2>/dev/null | while read -r key value; do
    key=${${key#branch.}%.wtnitem}
    (( ${+checked_out[$key]} )) && claimed[$value]=1
  done
  for row in ${(f)"$(items "$file")"}; do
    text=${${row#*$'\t'}%' (needs a choice)'}
    (( ${+claimed[$note: $text]} )) || print -r -- "$row"
  done
}

# Whether an item's plan is settled: "settled" when its `Plan: [[<name>]]`
# links a plan note (found by its file name anywhere in the notes directory
# but its dot-folders) with no "## Decision needed" section, the heading plans
# mark an open choice with, "choice" when the plan has one, and nothing when
# the item has no plan or its plan note isn't there.
plan_of() {
  local name=${${(M)1:#*Plan: \[\[*\]\]*}:+${${1#*Plan: \[\[}%%\]\]*}}
  [[ -n $name ]] || return 0
  local -a found
  found=(${NOTES_DIR:-$HOME/Notes}/**/$name.md(N.))
  (( ${#found} )) || return 0
  if grep -qiE '^#+[[:space:]]+decisions?[[:space:]]+needed' "${found[1]}"; then print choice; else print settled; fi
}

# The items wtn started in the last 30 minutes, from its claims file
# (bin/worktree-from-note), one a line, for the note at path $1 within the
# notes directory: taken even once their worktree is gone, since
# queue-and-clean may remove it a moment before it ticks the item.
claimed_lately() {
  local claims=$HOME/.local/state/switchboard/wtn-claims.tsv row
  [[ -r $claims ]] || return 0
  for row in ${(f)"$(<$claims)"}; do
    [[ ${row#*$'\t'} == "$1"$'\t'* ]] || continue
    (( EPOCHSECONDS - ${row%%$'\t'*} < 1800 )) && print -r -- "${row#*$'\t'*$'\t'}"
  done
}

# The items the mode would take, in its order (see the top).
queue() {
  local file=$1 ready_only=$2 row line text
  local -a settled unplanned
  local -A skip
  for text in ${(f)WTN_SKIP:-}; do skip[$text]=1; done
  for text in ${(f)"$(claimed_lately "${file#${NOTES_DIR:-$HOME/Notes}/}")"}; do skip[$text]=1; done
  for row in ${(f)"$(unclaimed "$file")"}; do
    [[ $row == *' (needs a choice)' ]] && continue
    line=${row%%$'\t'*} text=${row#*$'\t'}
    (( ${+skip[$text]} )) && continue
    # An empty item, as Notes makes one when Enter starts the next, is no work.
    [[ -n ${text//[[:space:]]/} ]] || continue
    case $(plan_of "$(item "$file" "$line")") in
      settled) settled+=("$row") ;;
      choice) ;;
      *) [[ -z $ready_only ]] && unplanned+=("$row") ;;
    esac
  done
  (( ${#settled} + ${#unplanned} )) && print -rl -- "${settled[@]}" "${unplanned[@]}"
  return 0
}

# A tab for one item: in the background, in this repository, named after the
# item and linked to its note, the note in WTN_NOTE and wtn typed with the
# item's line alone. Prints the window's id. The tab bar's name is the item's
# first 50 characters, cut back to a word and ending in …, and the item's
# whole first line is the tab's full title (@tab_title), which M-T's box, the
# sidebar and mission control show.
open_tab() {
  # Characters, not bytes, under any locale the caller has.
  setopt localoptions extendedglob
  local LC_ALL=en_US.UTF-8
  local note=$1 line=$2 name=$3 window short=$3
  if (( ${#name} > 50 )); then
    short=${name[1,49]}
    [[ $short == *' '* ]] && short=${short% *}
    short="${short%%[[:space:][:punct:]]#}…"
  fi
  # Two words, -t and the session: zsh doesn't split one expansion into two.
  window=$(tmux new-window -d ${WTN_SESSION:+-t} ${WTN_SESSION:+"$WTN_SESSION:"} -c "${PWD//\#/##}" -e "WTN_NOTE=$note" -n "wtn" -P -F '#{window_id}') || return 1
  tmux set-option -w -t "$window" @tab_name "$short"
  tmux set-option -w -t "$window" @tab_title "$name"
  tmux set-option -w -t "$window" @tab_note "$note"
  tmux send-keys -t "$window" "wtn --item $line" Enter
  print -r -- "$window"
}

if [[ ${1:-} == (queue|next) ]]; then
  file=${NOTES_DIR:-$HOME/Notes}/$2
  [[ -f $file ]] || { print -u2 "No note at $file"; exit 1; }
  if [[ $1 == queue ]]; then
    queue "$file" "${(M)3:#--ready-only}"
    exit
  fi
  count=${3:-1}
  for row in ${(f)"$(queue "$file" "${(M)4:#--ready-only}")"}; do
    (( count-- > 0 )) || break
    line=${row%%$'\t'*} text=${row#*$'\t'}
    window=$(open_tab "$2" "$line" "$text") || { print -u2 "tmux could not open a tab for: $text"; exit 1; }
    tmux set-option -w -t "$window" @autopilot "$EPOCHSECONDS"
    tmux set-option -w -t "$window" @autopilot_item "$text"
    print -r -- "$line"$'\t'"$window"$'\t'"$text"
  done
  exit 0
fi

if [[ ${1:-} == items ]]; then
  unclaimed "$2"
  exit
fi
if [[ ${1:-} == item ]]; then
  item "$2" "$3"
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

# Which repository, asked first (pick-dir.zsh), the focused pane's (the
# binding's -d) at the top, so Enter alone keeps it.
dir=$("${0:A:h}/pick-dir.zsh" --repos "Work on a note's items here: Enter lists your notes") || exit 0
cd "$dir" || exit 0

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

[[ -n $(items "$file") ]] || fail "$note has no unticked items."
list=$(unclaimed "$file")
[[ -n $list ]] || fail "Every unticked item of $note has a worktree already."
# The cursor's item whole, below the list. The note's path reaches the preview
# in the environment, never in its command line, which a shell reads.
preview=()
if [[ $(tmux show -gqv @setting_wtn_item_preview) != off ]]; then
  export WTN_PREVIEW_FILE=$file
  preview=(--preview "${(q)${0:A}} item \"\$WTN_PREVIEW_FILE\" {1}" --preview-window=down,50%,wrap,border-top)
fi
picked=$(
  print -r -- "$list" | sed "s/ (needs a choice)\$/ ${yellow}(needs a choice)${reset}/" |
    fzf --ansi --multi --delimiter=$'\t' --with-nth=2 "${preview[@]}" \
      --header='Enter: work on it in a new tab · Tab: pick several · Esc: cancel' \
      --prompt="${note:t}> " --layout=reverse --height=100%
) || exit 0

first=
for row in "${(@f)picked}"; do
  line=${row%%$'\t'*}
  [[ $line == <-> ]] || continue
  name=${${row#*$'\t'}//$'\e'\[[0-9;]#m/}
  name=${name%' (needs a choice)'}
  window=$(open_tab "$note.md" "$line" "$name") || fail "tmux could not open a tab for: $name"
  first=${first:-$window}
done
[[ -n $first ]] && tmux select-window -t "$first"
