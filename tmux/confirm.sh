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
  # The question in yellow, bold, as mission control's questions are
  # (confirmModal).
  printf "%*s\033[38;5;220;1m%s\033[0m" "$pad_left" "" "$text"
  # Yes and No under it, each taken by a click too (menu.sh, the menu_mouse
  # setting): the row they are on and the columns of each. The one Enter
  # takes has the ›, and Tab or Shift-Tab moves it, as on a webpage.
  . "${0%/*}/menu.sh"
  menu_mouse_on
  row=
  no=
  tabbed=
  yes_at=$(( (cols - 9) / 2 + 1 ))
  buttons() {
    if [ -n "$no" ]; then
      printf '\r\033[2K%*s  Yes \033[1m› No\033[0m' "$((yes_at - 3))" ""
    else
      printf '\r\033[2K%*s\033[1m› Yes\033[0m   No' "$((yes_at - 3))" ""
    fi
  }
  if [ -t 0 ]; then
    printf '\n\n'
    buttons
    printf '\033[1A\033[1A\r\033[%dC' "$((pad_left + ${#text}))"
    [ -n "${MENU_MOUSE:-}" ] && row=$(( $(menu_row) + 2 ))
  fi

  # Enter reads as an empty key and confirms too; a closed input does not.
  while :; do
    menu_read || { menu_mouse_off; exit 0; }
    case $MENU_KEY in
      enter) [ -n "$no" ] && ans=n || ans=y ;;
      # Space presses the one chosen once Tab has moved there; before, it is
      # any other key, so a stray Space never says yes.
      space) [ -n "$tabbed" ] && [ -z "$no" ] && ans=y || ans=n ;;
      none) continue ;;
      tab | btab)
        [ -t 0 ] || continue
        [ -n "$no" ] && no= || no=1
        tabbed=1
        printf '\033[2B'
        buttons
        printf '\033[2A\r\033[%dC' "$((pad_left + ${#text}))"
        continue
        ;;
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
