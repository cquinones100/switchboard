#!/usr/bin/env bash
# M-T: shows a tab's whole title in a box, since the sidebar cuts it at three
# lines and the status line at its width: the title as the tab bar has it
# (@tab_label, its PR number in front), or a note item's whole first line
# where the tab bar has it cut short (cut_of below), wrapped, and under it, faint, the
# branch, the directory, the PR and the Linear ticket, as far as they are
# known. Copy the title (or y) copies it; Close, Esc, q or a click off the
# options closes it.
#
# It reads tmux and path-facts.sh's cache alone, never the network, so it opens
# at once.
#
# Usage: tab-title.sh [-c <client>] <window>    the box, on that client
#        tab-title.sh --print <window>          what the box shows, as text
#        tab-title.sh box                       the box itself, in the popup

set -u
# tmux writes the tabs between fields as they are only under a UTF-8 locale,
# which the server's environment may not have.
export LC_ALL=en_US.UTF-8
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

if [ "${1:-}" = box ]; then
  . "${0%/*}/menu.sh"
  menu_colors
  # Copy and Close, moved through with j, k or Tab and taken with Enter or a
  # click, as every bash menu here is (menu.sh); y copies too, and Esc, q or
  # a click off the options closes it.
  options=("Copy the title" "Close")
  cursor=0
  menu_mouse_on
  while :; do
    printf '\033[H\033[2J\033[?25l\n'
    while IFS= read -r line; do printf '  \033[38;5;%s;1m%s\033[0m\n' "$MENU_BRIGHT" "$line"; done <<<"$TAB_TITLE_WRAPPED"
    if [ -n "$TAB_TITLE_FACTS" ]; then
      printf '\n'
      while IFS= read -r line; do printf '  \033[38;5;%sm%s\033[0m\n' "$MENU_DIM" "$line"; done <<<"$TAB_TITLE_FACTS"
    fi
    printf '\n'
    for i in "${!options[@]}"; do
      menu_option "$([ "$i" = "$cursor" ] && echo 1)" "${options[$i]}"
    done
    menu_keys "y copy  jk or tab move  ↵ or click choose  esc close"
    # Where the first option is, for a click's row (only on a terminal).
    start=
    [ -t 0 ] && [ -n "${MENU_MOUSE:-}" ] && start=$(( $(menu_row) - ${#options[@]} - 1 ))
    # The cursor past the options is on easy mode's close button (menu.sh).
    MENU_CLOSE_FOCUS=$([ "$cursor" = ${#options[@]} ] && echo 1)
    menu_read_menu "$start" ${#options[@]} "$cursor" || break
    [ -n "$MENU_CLOSE_FOCUS" ] && case $MENU_KEY in enter | space) MENU_KEY=quit ;; esac
    pick=
    case $MENU_KEY in
      j) [ $cursor -lt $((${#options[@]} - 1)) ] && cursor=$((cursor + 1)) ;;
      k) [ $cursor -gt 0 ] && cursor=$((cursor - 1)) ;;
      tab) cursor=$(( (cursor + 1) % $(menu_stops ${#options[@]}) )) ;;
      btab) cursor=$(( (cursor + $(menu_stops ${#options[@]}) - 1) % $(menu_stops ${#options[@]}) )) ;;
      hover\ *) cursor=${MENU_KEY#hover } ;;
      y) pick=0 ;;
      enter | space) pick=$cursor ;;
      click\ *)
        set -- $MENU_KEY
        i=$(( $2 - ${start:-0} ))
        if [ -n "$start" ] && [ "$i" -ge 0 ] && [ "$i" -lt ${#options[@]} ]; then pick=$i; else break; fi
        ;;
      none | move\ *) ;;
      *) break ;;
    esac
    [ -n "$pick" ] && break
  done
  menu_mouse_off
  if [ "${pick:-}" = 0 ]; then
    printf '%s' "$TAB_TITLE" | pbcopy
    "${0%/*}/say.sh" -a "Copied the tab's title"
  fi
  exit 0
fi

client="" print=""
while [ $# -gt 1 ]; do
  case $1 in
    -c) client=$2; shift 2 ;;
    --print) print=1; shift ;;
    *) break ;;
  esac
done
window=${1:-}
[ -n "$window" ] || exit 0

IFS=$'\t' read -r index label name tab_name tab_title item < <(tmux display -p -t "$window" $'#{window_index}\t#{?@tab_label,#{@tab_label},none}\t#{window_name}\t#{?@tab_name,#{@tab_name},none}\t#{?@tab_title,#{@tab_title},none}\t#{?@autopilot_item,#{@autopilot_item},none}' 2>/dev/null)
[ -n "${index:-}" ] || exit 0
[ "$label" = none ] && label=$name

# The directory the tab is named from (window-names.sh): its active pane's,
# past the sidebar and the notes pane M-e brings beside it, else any other's,
# and the notes pane's only in a tab with no other (Notes' own).
path="" notes=""
while IFS=$'\t' read -r sidebar note active dir; do
  [ "$sidebar" = 1 ] && continue
  if [ "$note" = 1 ]; then notes=$dir; continue; fi
  [ -n "$path" ] && [ "$active" != 1 ] && continue
  path=$dir
  [ "$active" = 1 ] && break
done < <(tmux list-panes -t "$window" -F $'#{?@sidebar,1,0}\t#{?@notes_pane,1,0}\t#{pane_active}\t#{pane_current_path}' 2>/dev/null)
[ -n "$path" ] || path=$notes

# The branch, PR and ticket as path-facts.sh last worked them out for the
# directory: its first line the time and branch, its second the color, the
# label with no name set (the ticket and its title when Linear knows it) and
# the PR. With nothing kept, the branch from git, and no PR or ticket.
branch="" pr="" ticket=""
if [ -n "$path" ]; then
  cache="${XDG_CACHE_HOME:-$HOME/.cache}/tmux-path-facts/${path//\//%}"
  if [ -f "$cache" ]; then
    { IFS=$'\t' read -r _ branch; IFS=$'\t' read -r _ facts_label pr; } <"$cache"
    [[ ${facts_label:-} =~ ^([A-Z][A-Z0-9]*-[0-9]+):\  ]] && ticket=${BASH_REMATCH[1]}
  else
    branch=$(git -C "$path" --no-optional-locks rev-parse --abbrev-ref HEAD 2>/dev/null)
  fi
fi
facts=""
add() { facts+="${facts:+$'\n'}$1"; }
[ -n "$branch" ] && [ "$branch" != HEAD ] && add "Branch     $branch"
tilde="~"
[ -n "$path" ] && add "Directory  ${path/#"$HOME"/$tilde}"
[ -n "${pr:-}" ] && add "PR         #$pr"
[ -n "$ticket" ] && add "Ticket     $ticket"

# A tab opened for a note's item has the item's first 50 characters as its
# name (@tab_name, wtn-open.zsh) and its whole first line as its full title:
# @tab_title, else, for a tab opened before there was one, the work-a-note
# mode's @autopilot_item, else the item its branch is for (wtnItem in git's
# config, "<note's ID>: <item's first line>", or "<note's path>: <item's
# first line>" from before notes had IDs). Each counts only while the name is
# still that item cut short, so a tab renamed since shows its new name.
cut_of() {
  local short=$1 whole=$2
  [ "$short" != none ] && [ -n "$short" ] && [ "$whole" != none ] && [ -n "$whole" ] || return 1
  [[ $short =~ ^#[0-9]+\  ]] && short=${short#* }
  # The name wtn-open.zsh gives: the item whole up to 50 characters, else its
  # first 49 cut back to a word, ending in …; or its first 50, as tabs opened
  # before were named.
  [ "$short" = "$whole" ] && return 0
  [ ${#whole} -gt 50 ] || return 1
  [ "$short" = "${whole:0:50}" ] && return 0
  local cut=${whole:0:49}
  [[ $cut == *" "* ]] && cut=${cut% *}
  shopt -s extglob
  cut=${cut%%+([[:space:][:punct:]])}
  [ "$short" = "${cut}…" ]
}
title=""
if cut_of "$tab_name" "$tab_title"; then title=$tab_title
elif cut_of "$tab_name" "$item"; then title=$item
elif [ -n "$path" ] && [ -n "$branch" ] && [ "$branch" != HEAD ]; then
  wtn_item=$(git -C "$path" --no-optional-locks config --get "branch.$branch.wtnItem" 2>/dev/null)
  case $wtn_item in
    n-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]': '*) wtn_item=${wtn_item#*: } ;;
    *) wtn_item=${wtn_item#*.md: } ;;
  esac
  cut_of "$tab_name" "$wtn_item" && title=$wtn_item
fi
if [ -n "$title" ]; then
  [[ $label =~ ^#[0-9]+\  ]] && title="${label%% *} $title"
  label=$title
fi

wrapped=$(printf '%s\n' "$label" | fold -s -w 68 | sed 's/ *$//')
if [ -n "$print" ]; then
  printf '%s\n' "$wrapped"
  [ -n "$facts" ] && printf '\n%s\n' "$facts"
  exit 0
fi

# As wide as what it shows, at most 72 columns inside, and as tall.
width=$(printf '%s\n%s\n' "$wrapped" "$facts" | awk '{ if (length > n) n = length } END { print n + 6 }')
# At least as wide as the keys line under the options (menu_keys: a space,
# then 52 columns), which would wrap otherwise and put a click's row off by one.
[ "$width" -lt 56 ] && width=56
rows=$(printf '%s\n' "$wrapped" | wc -l)
[ -n "$facts" ] && rows=$(( rows + 1 + $(printf '%s\n' "$facts" | wc -l) ))
# A blank row above, then a blank, the two options, a blank and the keys
# below, and the frame.
height=$(( rows + 8 ))
# A box already open on the client (say.sh's) would take this as a change to it.
"${0%/*}/say.sh" close ${client:+"$client"}
# The texts go in the environment, as they are, never in the command line; the
# title is a format, so its #s are doubled.
tmux display-popup ${client:+-c "$client"} -E -w "$width" -h "$height" -T "#[bold] Tab $index " \
  -e "TAB_TITLE=$label" -e "TAB_TITLE_WRAPPED=$wrapped" -e "TAB_TITLE_FACTS=$facts" "bash $(printf %q "$self") box" >/dev/null 2>&1 &
exit 0
