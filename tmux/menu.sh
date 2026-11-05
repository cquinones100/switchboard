#!/usr/bin/env bash
# What the bash menus in popups share (pr-ready.sh's answer, repo-update.sh's
# update prompt, confirm.sh's question): clicks while a menu waits, with the
# menu_mouse setting on, and one read that says what was pressed or clicked.
# Sourced, not run.

# Mouse reporting on (SGR, so a click says its row and column), and off again.
menu_mouse_on() {
  MENU_MOUSE=
  [ "$(tmux show -gqv @setting_menu_mouse 2>/dev/null)" = off ] && return 0
  printf '\033[?1000h\033[?1006h'
  MENU_MOUSE=1
}
menu_mouse_off() {
  [ -n "${MENU_MOUSE:-}" ] && printf '\033[?1000l\033[?1006l'
  MENU_MOUSE=
}

# The row the cursor is on now, from 1, asked of the terminal: where a menu's
# first option is, for a click's row to say which it chose.
menu_row() {
  local row col
  printf '\033[6n' >/dev/tty
  IFS='[;' read -rsd R _ row col </dev/tty
  printf '%s' "$row"
}

# One key or click, into MENU_KEY: j or k (the arrows and the wheel too),
# enter, quit (Esc or q), "click <row> <col>" for a left click, none for a
# mouse event that means nothing (a release), else the key itself. Returns 1
# at the end of the input.
menu_read() {
  local key c ch seq b x y
  IFS= read -rsn 1 key || return 1
  case $key in
    '') MENU_KEY=enter; return 0 ;;
    q) MENU_KEY=quit; return 0 ;;
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
    '<')
      seq=
      while IFS= read -rsn 1 -t 1 ch; do
        case $ch in M | m) break ;; esac
        seq+=$ch
      done
      IFS=';' read -r b x y <<<"$seq"
      MENU_KEY=none
      # Digits only, since callers do arithmetic with them.
      if [ "$ch" = M ] && [[ $x =~ ^[0-9]+$ && $y =~ ^[0-9]+$ ]]; then
        case $b in
          0) MENU_KEY="click $y $x" ;;
          64) MENU_KEY=k ;;
          65) MENU_KEY=j ;;
        esac
      fi
      ;;
  esac
  return 0
}
