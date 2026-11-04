#!/usr/bin/env bash

# Shapes a note's to-dos as tickets, in Linear or Jira, for the M-p action: pick a note,
# the checklist items to shape (every unticked one, to start with), and the
# repo they belong to, and each item gets its own tab running /shape, through
# shape-batch. /shape looks for a ticket that already covers an item first,
# and offers to resume shaping that one instead.
#
# Usage: note-shape.sh          (from a popup; the current tab's repo is offered first)
#        note-shape.sh prompt   one new ticket from an idea typed in the popup
#
# The notes are the notes app's (bin/notes): Markdown in $NOTES_DIR or ~/Notes,
# the one open in the app first. An item is an unticked top-level "- [ ]"
# line, and everything indented under it is part of its ticket: lines that
# aren't checklist items are its details, and unticked checklist items at the
# first level under it are its sub-items, each a ticket of its own, with
# anything indented under them as their details. A ticked item, and all that
# is under it, is done and left out, and so is one struck through (~~...~~). /shape gets the item on one line, with
# the note's title, its details and its sub-items labelled, and asks first
# whether an item with sub-items should become a project (a Jira epic) with
# them as its issues.

set -u
export LC_ALL=en_US.UTF-8

fail() {
  printf '\n  %s\n\n  Press any key to close.' "$1"
  read -rsn 1
  exit 0
}

if [ "${1:-}" = prompt ]; then
  # M-p's "Shape a new ticket": one idea, typed here, in place of a note's items.
  printf '\n  What should the ticket be about? (Enter on nothing cancels)\n\n'
  IFS= read -e -r -p '  > ' chosen || exit 0
  chosen=$(printf '%s' "$chosen" | sed -E 's/^[[:space:]]+//; s/[[:space:]]+$//')
  [ -n "$chosen" ] || exit 0
else
  notes=${NOTES_DIR:-$HOME/Notes}
  [ -d "$notes" ] || fail "No notes directory at $notes."

  # The note open in the app (its last), then the rest, newest first.
  open_note=$(jq -r '.last // empty' "$notes/.notes.json" 2>/dev/null)
  note=$(
    cd "$notes" &&
      { [ -n "$open_note" ] && [ -f "$open_note" ] && printf '%s\n' "$open_note"
        find . -name '*.md' -not -path './.trash/*' -print0 | xargs -0 ls -t 2>/dev/null | sed 's#^\./##' | grep -vxF -- "$open_note"; } |
      fzf --layout=reverse --border=rounded --height=100% --prompt='note> ' \
        --header='The note whose checklist to shape' --preview "grep -E '^[[:space:]]*- \\[ \\] ' {} | head -40"
  )
  [ -n "$note" ] || exit 0
  title=$(head -1 "$notes/$note" | sed -E 's/^#+[[:space:]]*//')
  [ -n "$title" ] || title=${note%.md}

  # One line per unticked top-level item: what the picker shows, a tab, and
  # what /shape gets (see the top).
  items=$(awk -v title="$title" '
    function flush(   out, i) {
      if (item == "") return
      sub(/[.]+$/, "", details)
      out = item " (from my note \"" title "\")."
      if (details != "") out = out " Details: " details "."
      if (subs > 0) {
        out = out " Sub-items, each its own ticket:"
        for (i = 1; i <= subs; i++) out = out " (" i ") " child[i] (childdetails[i] != "" ? " [" childdetails[i] "]" : "") (i < subs ? ";" : ".")
      }
      printf "%s%s\t%s\n", item, (subs > 0 ? "  (" subs " sub-item" (subs > 1 ? "s" : "") ")" : ""), out
    }
    function add(list, text) { return list == "" ? text : list " / " text }
    /^[[:space:]]*$/ { next }
    {
      match($0, /^[[:space:]]*/); depth = RLENGTH
      checklist = ($0 ~ /^[[:space:]]*- \[[ xX]\]/)
      text = $0; sub(/^[[:space:]]*(- \[[ xX]\]|[-*+]|[0-9]+\.)?[[:space:]]*/, "", text)
      # Struck through (~~...~~ at the start) is dropped, the same as ticked.
      ticked = ($0 ~ /^[[:space:]]*- \[[xX]\]/) || (checklist && text ~ /^~~/)
    }
    # A top-level line ends the item before it; an unticked checklist one starts the next.
    depth == 0 {
      flush(); item = ""; details = ""; subs = 0; split("", child); split("", childdetails); childdepth = 0; skipping = 0
      if (checklist && !ticked && text != "") item = text
      next
    }
    item == "" { next }
    # Everything under a ticked sub-item is done with it.
    skipping && depth > childdepth { next }
    { skipping = 0 }
    # The first checklist level under the item holds its sub-items.
    checklist && (childdepth == 0 || depth <= childdepth) {
      childdepth = depth
      if (ticked) { skipping = 1; next }
      if (text != "") { child[++subs] = text; childdetails[subs] = "" }
      next
    }
    subs > 0 && depth > childdepth { childdetails[subs] = add(childdetails[subs], text); next }
    { details = add(details, text) }
    END { flush() }
  ' "$notes/$note")
  [ -n "$items" ] || fail "\"$title\" has no unticked checklist items."

  chosen=$(
    printf '%s\n' "$items" |
      fzf --multi --bind 'load:select-all' --delimiter=$'\t' --with-nth=1 --layout=reverse --border=rounded --height=100% \
        --prompt='shape> ' --header="Tab deselects an item; Enter shapes the selected, one tab each" |
      cut -f2
  )
  [ -n "$chosen" ] || exit 0
fi

# The repos the tabs are in, as their main checkout (a worktree counts as its
# repo), the current tab's first.
repo_of() { local common; common=$(git -C "$1" rev-parse --path-format=absolute --git-common-dir 2>/dev/null) && dirname "$common"; }
here=$(repo_of "$PWD")
repo=$(
  { [ -n "$here" ] && echo "$here"
    tmux list-panes -a -F '#{pane_current_path}' | sort -u | while IFS= read -r dir; do repo_of "$dir"; done | sort -u | grep -vxF -- "${here:-/}"; } |
    fzf --layout=reverse --border=rounded --height=100% --prompt='repo> ' --header='The repo these tickets are for'
)
[ -n "$repo" ] || exit 0

# shape-batch is node, and the tmux server has no node on its PATH (see
# sidebar.sh's start).
if ! command -v node >/dev/null 2>&1; then
  # nvm installs into ~/.nvm, or under XDG_CONFIG_HOME when that was set.
  for NVM_DIR in ${NVM_DIR:+"$NVM_DIR"} "$HOME/.nvm" "${XDG_CONFIG_HOME:-$HOME/.config}/nvm"; do
    [ -s "$NVM_DIR/nvm.sh" ] && break
  done
  default=$(cat "$NVM_DIR/alias/default" 2>/dev/null)
  candidate="$NVM_DIR/versions/node/v${default#v}/bin"
  if [ -n "$default" ] && [ -x "$candidate/node" ]; then
    export PATH="$candidate:$PATH"
  elif [ -s "$NVM_DIR/nvm.sh" ]; then
    . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
  fi
fi

args=()
while IFS= read -r item; do args+=("$item"); done <<<"$chosen"
cd "$repo" || fail "Can't reach $repo."
~/switchboard/bin/shape-batch "${args[@]}" || fail "shape-batch failed."
sleep 1.5
