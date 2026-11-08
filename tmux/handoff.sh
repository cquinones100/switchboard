#!/usr/bin/env bash
# M-p's "Hand this workspace to the Rust port" (no argument) and "Bring the
# workspace back from the Rust port" (back): bin/switchboard-handoff's dry run
# in the popup, then a menu whose first option, which Enter takes, runs it.
# Esc, q, a click outside the options or the end of the input runs nothing.
# A popup's PATH has no nvm, so the script runs under an interactive zsh, as
# habits-report.sh does.

. "${0%/*}/menu.sh"

export HANDOFF="${SWITCHBOARD_DIR:-$HOME/switchboard}/bin/switchboard-handoff"
args=()
yes="Hand it off now"
if [ "${1:-}" = back ]; then
  args=(--back)
  yes="Bring it back now"
fi
handoff() { zsh -i +m -c '"$HANDOFF" "$@"' handoff "$@"; }
close() {
  printf '\n  \033[38;5;244mPress any key to close.\033[0m'
  menu_any_key
  exit 0
}

printf '\n'
handoff "${args[@]}" --dry-run || close

# The two options and the keys under them, drawn once to make room, then
# again in place as the cursor moves: the options start three rows above the
# keys' row.
cursor=0
draw() {
  menu_option "$([ "$cursor" = 0 ] && echo 1)" "$yes"
  menu_option "$([ "$cursor" = 1 ] && echo 1)" "Not now"
  menu_keys
}
printf '\n'
draw
start=$(( $(menu_row) - 3 ))
answered=
menu_mouse_on
# The cursor past the options is on easy mode's close button (menu.sh), a Tab
# stop whose Enter closes the popup.
while MENU_CLOSE_FOCUS=$([ "$cursor" = 2 ] && echo 1); menu_read_menu "$start" 2 "$cursor"; do
  [ -n "$MENU_CLOSE_FOCUS" ] && case $MENU_KEY in enter | space) MENU_KEY=quit ;; esac
  case $MENU_KEY in
    j | k) cursor=$(( cursor == 0 ? 1 : 0 )) ;;
    tab) cursor=$(( (cursor + 1) % $(menu_stops 2) )) ;;
    btab) cursor=$(( (cursor + $(menu_stops 2) - 1) % $(menu_stops 2) )) ;;
    hover\ *) cursor=${MENU_KEY#hover } ;;
    enter | space) answered=1; break ;;
    y) cursor=0; answered=1; break ;;
    n | quit) break ;;
    click\ *)
      row=${MENU_KEY#click }
      row=$(( ${row%% *} - start ))
      [ "$row" -ge 0 ] && [ "$row" -lt 2 ] && cursor=$row && answered=1
      break
      ;;
  esac
  printf '\033[%sH' "$start"
  draw
done
menu_mouse_off
[ -n "$answered" ] && [ "$cursor" = 0 ] || exit 0

printf '\033[2J\033[H\n'
handoff "${args[@]}"
close
