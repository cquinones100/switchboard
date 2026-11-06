#!/usr/bin/env bash
# What the bash menus in popups share (pr-ready.sh's answer, repo-update.sh's
# update prompt, confirm.sh's question): clicks while a menu waits, with the
# menu_mouse setting on, easy mode's close button, and one read that says
# what was pressed or clicked. Sourced, not run.

# Mouse reporting on (SGR, so a click says its row and column), and off again.
# With the hover_highlight setting on, the mouse's every move too (1003, which
# replaces 1000's clicks alone), for what is under it to light (menu_read's
# move).
menu_mouse_on() {
  MENU_MOUSE=
  [ "$(tmux show -gqv @setting_menu_mouse 2>/dev/null)" = off ] && return 0
  printf '\033[?1000h\033[?1006h'
  [ "$(tmux show -gqv @setting_hover_highlight 2>/dev/null)" = off ] || printf '\033[?1003h'
  MENU_MOUSE=1
  menu_close_on
}
menu_mouse_off() {
  [ -n "${MENU_MOUSE:-}" ] && printf '\033[?1003l\033[?1000l\033[?1006l'
  MENU_MOUSE=
  MENU_CLOSE_HOVER=
}

# Easy mode's close button (the easy_keys setting), as mission control's
# boxes have one (render.js's CLOSE): " × " at the right end of the popup's
# top row, a cell short of its edge. MENU_CLOSE is the column it starts at,
# empty when it isn't drawn; menu_read draws it again before each read, since
# a menu drawing over the top row can wipe it, and takes a click on it as Esc.
# It is a Tab stop after a menu's options: MENU_CLOSE_FOCUS=1 draws it in
# reverse video, as mission control draws focus, and menu_stops counts it;
# MENU_CLOSE_HOVER=1, which menu_read sets while the mouse is over it, too.
menu_close_on() {
  MENU_CLOSE=
  [ -t 0 ] && [ "$(tmux show -gqv @setting_easy_keys 2>/dev/null)" = on ] || return 0
  MENU_CLOSE=$(( $(tput cols 2>/dev/null || echo 0) - 3 ))
  [ "$MENU_CLOSE" -gt 0 ] || MENU_CLOSE=
}
menu_close_draw() {
  [ -n "${MENU_CLOSE:-}" ] || return 0
  [ -n "${MENU_BRIGHT:-}" ] || menu_colors
  printf '\0337\033[1;%dH%s\033[38;5;%s;1m × \033[0m\0338' "$MENU_CLOSE" "$([ "${MENU_CLOSE_FOCUS:-}${MENU_CLOSE_HOVER:-}" != "" ] && printf '\033[7m')" "$MENU_BRIGHT"
}
# How many stops Tab goes round in a menu of <n> options: the options, and
# the close button after them when it is drawn.
menu_stops() { echo $(( $1 + ($([ -n "${MENU_CLOSE:-}" ] && echo 1 || echo 0)) )); }

# "Press any key to close": one key or click, the close button drawn while
# it waits, and the rest of a click read too, so nothing is left for what runs
# after. Returns 1 at the end of the input, as read does.
# The mouse moving (or a click's release) isn't a key, and is read past.
menu_any_key() {
  local status=0
  menu_mouse_on
  while :; do
    menu_read || { status=1; break; }
    case $MENU_KEY in move\ * | none) continue ;; esac
    break
  done
  menu_mouse_off
  return $status
}

# The row the cursor is on now, from 1, asked of the terminal: where a menu's
# first option is, for a click's row to say which it chose. The mouse's moves
# (hover_highlight) can come in ahead of the answer, ESC [ row ; col R, so
# only what follows the last [ is read as it.
menu_row() {
  local reply
  printf '\033[6n' >/dev/tty
  IFS= read -rsd R reply </dev/tty
  menu_row_of "$reply"
}
# The row in a cursor-position answer read up to its R.
menu_row_of() {
  local reply=${1##*[}
  printf '%s' "${reply%%;*}"
}

# One key or click, into MENU_KEY: j or k (the arrows and the wheel too),
# tab or btab (Tab and Shift-Tab, which go round the options, as on a webpage),
# space (which presses the option chosen, as Enter does), enter, quit (Esc or q), "click <row> <col>" for a left click, none for a
# mouse event that means nothing (a release), "move <row> <col>" for the
# mouse moving (hover_highlight), which lights the close button under it,
# else the key itself; a click on the close button is quit. Returns 1 at the
# end of the input.
menu_read() {
  local key c ch seq b x y
  menu_close_draw
  IFS= read -rsn 1 key || return 1
  # Any key puts out the close button the mouse lit, until it next moves.
  MENU_CLOSE_HOVER=
  case $key in
    '') MENU_KEY=enter; return 0 ;;
    q) MENU_KEY=quit; return 0 ;;
    $'\t') MENU_KEY=tab; return 0 ;;
    ' ') MENU_KEY=space; return 0 ;;
  esac
  if [ "$key" != $'\x1b' ]; then
    MENU_KEY=$key
    return 0
  fi
  # Esc alone sends nothing after it; an arrow sends [ A or [ B; a click
  # [ < button ; column ; row M (m for its release).
  MENU_KEY=quit
  IFS= read -rsn 1 -t 1 c || return 0
  [ "$c" = '[' ] || return 0
  IFS= read -rsn 1 -t 1 c || return 0
  case $c in
    A) MENU_KEY=k ;;
    B) MENU_KEY=j ;;
    Z) MENU_KEY=btab ;;
    '<')
      seq=
      while IFS= read -rsn 1 -t 1 ch; do
        case $ch in M | m) break ;; esac
        seq+=$ch
      done
      IFS=';' read -r b x y <<<"$seq"
      MENU_KEY=none
      # Digits only, since callers do arithmetic with them.
      if [ "$ch" = M ] && [[ $b =~ ^[0-9]+$ && $x =~ ^[0-9]+$ && $y =~ ^[0-9]+$ ]] && [ "$b" -ge 32 ] && [ "$b" -lt 64 ]; then
        MENU_KEY="move $y $x"
        [ -n "${MENU_CLOSE:-}" ] && [ "$y" = 1 ] && [ "$x" -ge "$MENU_CLOSE" ] && [ "$x" -le $((MENU_CLOSE + 2)) ] && MENU_CLOSE_HOVER=1
      elif [ "$ch" = M ] && [[ $x =~ ^[0-9]+$ && $y =~ ^[0-9]+$ ]]; then
        case $b in
          0)
            if [ -n "${MENU_CLOSE:-}" ] && [ "$y" = 1 ] && [ "$x" -ge "$MENU_CLOSE" ] && [ "$x" -le $((MENU_CLOSE + 2)) ]; then
              MENU_KEY=quit
            else
              MENU_KEY="click $y $x"
            fi
            ;;
          64) MENU_KEY=k ;;
          65) MENU_KEY=j ;;
        esac
      fi
      ;;
  esac
  return 0
}

# menu_read for a menu of <count> options, the first on row <start> (empty
# when unknown) and <cursor> chosen: the mouse moving onto another option is
# "hover <i>", which the menu takes as moving its cursor there, as mission
# control's menus do; any other move is read past, the close button lit or
# put out, so the menu is drawn again only when what is under the mouse
# changes. Returns 1 at the end of the input.
menu_read_menu() {
  local start=$1 count=$2 cursor=$3 row i
  while menu_read; do
    case $MENU_KEY in
      move\ *)
        row=${MENU_KEY#move }
        row=${row%% *}
        i=$(( row - ${start:-0} ))
        if [ -n "$start" ] && [ "$i" -ge 0 ] && [ "$i" -lt "$count" ] && [ "$i" != "$cursor" ]; then
          MENU_KEY="hover $i"
          return 0
        fi
        ;;
      *) return 0 ;;
    esac
  done
  return 1
}

# The colors mission control's boxes use (lib/sidebar/text.js's PALETTES),
# for the mode theme.sh last set: MENU_TEXT, MENU_DIM, MENU_BRIGHT and
# MENU_HIGHLIGHT, as 256-color numbers.
menu_colors() {
  if [ "$(cat "$HOME/.config/tmux/.theme-mode" 2>/dev/null)" = light ]; then
    MENU_TEXT=236 MENU_DIM=242 MENU_BRIGHT=232 MENU_HIGHLIGHT=252
  else
    MENU_TEXT=250 MENU_DIM=244 MENU_BRIGHT=255 MENU_HIGHLIGHT=238
  fi
}

# One option of a menu, as mission control's boxes draw one (chooseModal):
# the chosen one on the highlight across the whole row, a yellow › and its
# label bright and bold; the others in the text color. Clears the row first.
# Usage: menu_option <1 if chosen> <label>
menu_option() {
  [ -n "${MENU_TEXT:-}" ] || menu_colors
  if [ "$1" = 1 ]; then
    printf '\r\033[2K\033[48;5;%sm\033[K \033[38;5;220;1m›\033[0m\033[48;5;%sm \033[38;5;%s;1m%s\033[0m\n' "$MENU_HIGHLIGHT" "$MENU_HIGHLIGHT" "$MENU_BRIGHT" "$2"
  else
    printf '\r\033[2K   \033[38;5;%sm%s\033[0m\n' "$MENU_TEXT" "$2"
  fi
}

# The keys under a menu, dim, after a blank line, as the boxes end.
menu_keys() {
  [ -n "${MENU_DIM:-}" ] || menu_colors
  printf '\n \033[38;5;%sm%s\033[0m' "$MENU_DIM" "${1:-jk or tab move  ↵ or click choose  esc back}"
}
