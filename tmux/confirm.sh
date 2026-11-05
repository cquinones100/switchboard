#!/usr/bin/env bash
# Popup confirmation for tmux pane actions.
# Usage: confirm.sh "Prompt text" [<pane_id>]
#        confirm.sh --yes [<pane_id>]   closes it without asking, for M-w with
#                                       the confirm_close setting off
#
# Without a pane, the one in the global @confirm_pane, which M-w sets just
# before the popup, since display-popup does not expand #{pane_id} in its
# command.

prompt="$1"
target=${2:-$(tmux show -gqv @confirm_pane)}
[ -n "$target" ] || exit 0

if [ "$prompt" = --yes ]; then
  ans=y
else
  text="$prompt [Y/n] "
  cols=$(tput cols)
  lines=$(tput lines)
  pad_left=$(( (cols - ${#text}) / 2 ))
  pad_top=$(( (lines - 1) / 2 ))

  for ((i = 0; i < pad_top; i++)); do echo; done
  printf "%*s%s" "$pad_left" "" "$text"
  # Yes and No under it, each taken by a click too (menu.sh, the menu_mouse
  # setting): the row they are on and the columns of each.
  . "${0%/*}/menu.sh"
  menu_mouse_on
  row=
  if [ -n "${MENU_MOUSE:-}" ] && [ -t 0 ]; then
    yes_at=$(( (cols - 9) / 2 + 1 ))
    printf '\n\n%*s\033[1mYes\033[0m   No\033[1A\033[1A\r\033[%dC' "$((yes_at - 1))" "" "$((pad_left + ${#text}))"
    row=$(( $(menu_row) + 2 ))
  fi

  # Enter reads as an empty key and confirms too; a closed input does not.
  while :; do
    menu_read || { menu_mouse_off; exit 0; }
    case $MENU_KEY in
      enter) ans=y ;;
      none) continue ;;
      click\ *)
        set -- $MENU_KEY
        [ -n "$row" ] && [ "$2" = "$row" ] || continue
        if [ "$3" -ge "$yes_at" ] && [ "$3" -lt $((yes_at + 3)) ]; then ans=y
        elif [ "$3" -ge $((yes_at + 6)) ] && [ "$3" -lt $((yes_at + 8)) ]; then ans=n
        else continue
        fi
        ;;
      *) ans=$MENU_KEY ;;
    esac
    break
  done
  menu_mouse_off
fi

case "$ans" in
  ""|y|Y)
    # A pane standing in for another (pane-in-place.sh) puts it back first, so
    # closing it never leaves that pane parked out of sight.
    parked=$(tmux show -pqv -t "$target" @parked_pane 2>/dev/null)
    [ -n "$parked" ] && tmux swap-pane -d -s "$parked" -t "$target" 2>/dev/null && tmux select-pane -t "$parked"
    tmux kill-pane -t "$target"
    ;;
esac
