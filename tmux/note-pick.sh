#!/usr/bin/env bash
# Asks which note a tab's work belongs to and records it on the tab
# (@tab_note, a path in the notes directory), for M-e to show beside it
# (tmux/notes-open.sh). The likely ones come first, from what the tab is
# about: notes that mention its ticket or PR number, then those whose title
# shares words with the tab's name, then the note last shown beside it, then
# those that mention its repository's name, then the rest, most recently
# changed first. The notes are searched here, on the Mac, and sent nowhere.
#
# Usage:
#   note-pick.sh ask <window> <client>   in a popup on <client>; M-e runs it
#                                        for a tab with no note yet, and M-p's
#                                        "Choose this tab's note" any time
#   note-pick.sh list <window>           the ranked list, a "<title>\t<path>"
#                                        line each (the tests read it)
#
# "Keep whatever is open" records that the tab was asked (@tab_note_asked), so
# M-e doesn't ask again; Esc leaves it to ask next time.

set -u
notes_dir=${NOTES_DIR:-$HOME/Notes}
export LC_ALL=en_US.UTF-8

list() {
  local window=$1 label dir repo terms=() last
  IFS=$'\t' read -r label dir last < <(tmux display -p -t "$window" $'#{@tab_label}\t#{pane_current_path}\t#{@tab_note_last}')
  # What the tab is about: its ticket ID and PR number from its name, the
  # words of its name, and its repository's name.
  terms+=($(printf '%s' "$label" | grep -oE '[A-Z][A-Z0-9]*-[0-9]+|#[0-9]+' | sort -u))
  repo=$(basename "$(git -C "$dir" rev-parse --path-format=absolute --git-common-dir 2>/dev/null | sed 's#/\.git$##')" 2>/dev/null)
  {
    # Ranked, lowest first: 0, a note that mentions the tab's ticket or PR;
    # 1.x, one whose title shares words with the tab's name, the more the
    # better; 2, the note last shown beside it; 3, one that mentions its
    # repository; 4, the rest. Within a rank, the most recently changed first.
    local term
    for term in ${terms[@]+"${terms[@]}"}; do
      grep -rlFi --include='*.md' --exclude-dir=.trash -- "$term" "$notes_dir" 2>/dev/null | sed "s#^#0\t#"
    done
    [ -n "$last" ] && [ -f "$notes_dir/$last" ] && printf '2\t%s\n' "$notes_dir/$last"
    [ -n "$repo" ] && [ "$repo" != . ] && grep -rlFi --include='*.md' --exclude-dir=.trash -- "$repo" "$notes_dir" 2>/dev/null | sed 's#^#3\t#'
    find "$notes_dir" -name '*.md' -not -path '*/.trash/*' -not -name '.*' -print0 2>/dev/null |
      xargs -0 stat -f $'%m\t%N' 2>/dev/null | sort -rn | cut -f2- |
      awk -v label="$label" '
        BEGIN {
          split("a an and the of for to in on with at by is it my our", stop, " ")
          for (i in stop) skip[stop[i]] = 1
          n = split(tolower(label), w, /[^a-z0-9]+/)
          for (i = 1; i <= n; i++) if (length(w[i]) > 1 && !(w[i] in skip)) want[w[i]] = 1
        }
        {
          title = $0; sub(/.*\//, "", title); sub(/\.md$/, "", title)
          delete has; m = split(tolower(title), t, /[^a-z0-9]+/)
          for (i = 1; i <= m; i++) has[t[i]] = 1
          c = 0; for (x in want) if (x in has) c++
          if (c) printf "%.1f\t%s\n", 2 - (c > 9 ? 9 : c) / 10, $0
          printf "4\t%s\n", $0
        }'
  } | sort -s -t$'\t' -k1,1n | awk -F'\t' '!seen[$2]++' | cut -f2- |
    while IFS= read -r file; do
      rel=${file#"$notes_dir"/}
      printf '%s\t%s\n' "${rel%.md}" "$rel"
    done
}
case "${1:-}" in
  list)
    list "${2:?window}"
    ;;

  ask)
    window=${2:?window} client=${3:-}
    # Only with a client to ask on; the tests have none.
    tmux display -p -c "$client" '#{client_name}' >/dev/null 2>&1 || exit 0
    choices=$(mktemp) answer=$(mktemp)
    trap 'rm -f "$choices" "$answer"' EXIT
    # The likeliest note first, so Enter takes it; keeping what is open last.
    { list "$window"; printf 'Keep whatever is open\t\n'; } >"$choices"
    name=$(tmux display -p -t "$window" '#{@tab_label}')
    # Its list looks like mission control's boxes (fzf-style.sh).
    tmux display-popup -c "$client" -e "FZF_DEFAULT_OPTS=${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${0%/*}/fzf-style.sh" 2>/dev/null)" -E -w 70 -h 60% -T "#[bold] Which note is this tab's? " \
      "fzf --delimiter '\t' --with-nth 1 --layout reverse --prompt 'Note: ' --header '${name//\'/} · Esc asks again next time' <'$choices' >'$answer'"
    choice=$(cut -f2 "$answer")
    if [ -s "$answer" ] && [ -z "$choice" ]; then
      tmux set-option -w -t "$window" @tab_note_asked 1
    elif [ -n "$choice" ]; then
      tmux set-option -w -t "$window" @tab_note "$choice"
      # Notes beside the tab already shows it now.
      "${0%/*}/notes-open.sh" show "$window"
    fi
    ;;
esac
exit 0
