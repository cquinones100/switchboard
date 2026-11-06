#!/usr/bin/env bash
# Popup confirmation for tmux pane actions.
# Usage: confirm.sh "Prompt text" [<pane_id>]
#        confirm.sh --yes [<pane_id>]   closes it without asking, for M-w with
#                                       the confirm_close setting off
#        confirm.sh --run "Prompt text" <command> [<arg>...]
#                                       runs the command on a yes, in place
#                                       of closing a pane
#
# Without a pane, the one in the global @confirm_pane, which M-w sets just
# before the popup, since display-popup does not expand #{pane_id} in its
# command.

run=()
if [ "${1:-}" = --run ]; then
  prompt=$2
  shift 2
  run=("$@")
  [ ${#run[@]} -gt 0 ] || exit 0
else
  prompt="$1"
  target=${2:-$(tmux show -gqv @confirm_pane)}
  [ -n "$target" ] || exit 0
fi

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
  onclose=
  tabbed=
  lit=
  yes_at=$(( (cols - 9) / 2 + 1 ))
  # One of them: › and bold when Enter takes it (<chosen> 1), in reverse
  # video while the mouse is over it (<lit> 1, hover_highlight), which
  # Enter doesn't take.
  button() {
    printf '%s%s%s\033[0m' "$([ "$2" = 1 ] && printf '\033[1m› ' || printf '  ')" "$([ "$3" = 1 ] && printf '\033[7m')" "$1"
  }
  buttons() {
    printf '\r\033[2K%*s' "$((yes_at - 3))" ""
    button Yes "$([ -z "$no$onclose" ] && echo 1)" "$([ "$lit" = yes ] && echo 1)"
    printf ' '
    button No "$([ -n "$no" ] && echo 1)" "$([ "$lit" = no ] && echo 1)"
  }
  # Yes and No drawn again where they are, from the question's line.
  redraw() {
    printf '\033[2B'
    buttons
    printf '\033[2A\r\033[%dC' "$((pad_left + ${#text}))"
  }
  # Which of them a column on their row is: yes, no or nothing.
  under() {
    if [ "$1" -ge "$yes_at" ] && [ "$1" -lt $((yes_at + 3)) ]; then echo yes
    elif [ "$1" -ge $((yes_at + 6)) ] && [ "$1" -lt $((yes_at + 8)) ]; then echo no
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
    # Easy mode's close button (menu.sh) is a Tab stop after No, pressed as no.
    MENU_CLOSE_FOCUS=$onclose
    menu_read || { menu_mouse_off; exit 0; }
    # Any key puts out what the mouse lit.
    case $MENU_KEY in move\ *) ;; *) [ -n "$lit" ] && [ -t 0 ] && { lit=; redraw; } ;; esac
    case $MENU_KEY in
      enter) [ -n "$no$onclose" ] && ans=n || ans=y ;;
      # Space presses the one chosen once Tab has moved there; before, it is
      # any other key, so a stray Space never says yes.
      space) [ -n "$tabbed" ] && [ -z "$no$onclose" ] && ans=y || ans=n ;;
      none) continue ;;
      move\ *)
        set -- $MENU_KEY
        now=
        [ -n "$row" ] && [ "$2" = "$row" ] && now=$(under "$3")
        [ "$now" != "$lit" ] && { lit=$now; redraw; }
        continue
        ;;
      tab | btab)
        [ -t 0 ] || continue
        if [ -n "${MENU_CLOSE:-}" ]; then
          # Yes, No, then the close button, round in either direction.
          if [ "$MENU_KEY" = tab ]; then
            if [ -n "$no" ]; then no= onclose=1; elif [ -n "$onclose" ]; then onclose=; else no=1; fi
          else
            if [ -n "$no" ]; then no=; elif [ -n "$onclose" ]; then onclose= no=1; else onclose=1; fi
          fi
        else
          [ -n "$no" ] && no= || no=1
        fi
        tabbed=1
        redraw
        continue
        ;;
      click\ *)
        set -- $MENU_KEY
        [ -n "$row" ] && [ "$2" = "$row" ] || continue
        case $(under "$3") in
          yes) ans=y ;;
          no) ans=n ;;
          *) continue ;;
        esac
        ;;
      *) ans=$MENU_KEY ;;
    esac
    break
  done
  menu_mouse_off
fi

case "$ans" in
  ""|y|Y)
    [ ${#run[@]} -gt 0 ] && exec "${run[@]}"
    # A pane standing in for another (pane-in-place.sh) puts it back first, so
    # closing it never leaves that pane parked out of sight.
    parked=$(tmux show -pqv -t "$target" @parked_pane 2>/dev/null)
    [ -n "$parked" ] && tmux swap-pane -d -s "$parked" -t "$target" 2>/dev/null && tmux select-pane -t "$parked"
    tmux kill-pane -t "$target"
    ;;
esac
