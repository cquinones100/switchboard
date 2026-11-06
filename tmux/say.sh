#!/usr/bin/env bash
# Says something in a small box in the middle of the screen, in place of
# tmux's message on the status line, which is easy to miss and gone in a
# moment: any key closes it, and it closes itself after a few seconds. With
# the message_boxes setting off, it is the status-line message again.
#
# A client shows one popup at a time, so a script that says something and
# then opens a popup of its own closes the box first (say.sh close), and one
# running in a popup says it once that popup has closed (-a, after).
#
# Usage: say.sh [-c <client>] [-t <seconds>] [-a] <text>
#        say.sh close [<client>]          close a box this script opened
#                                         there (the client used last)
#        say.sh wait [<client>]           wait until that box has closed
#        say.sh box [<client>]            the box itself, its text and
#                                         time in SAY_TEXT and SAY_SECONDS

set -u

# A box asked for but not yet up, for its client: a file made before the
# popup is asked for and gone once the box runs, so a close that comes in
# between waits for it rather than missing it.
pending() { printf '%s/say-pending-%s' "${XDG_STATE_HOME:-$HOME/.local/state}/switchboard" "${1//[^a-zA-Z0-9]/_}"; }

if [ "${1:-}" = box ]; then
  rm -f "$(pending "${2:-}")"
  printf '\033[?25l\n'
  printf '%s\n' "$SAY_TEXT" | sed 's/^/  /'
  # Any key closes it, or a click, or the time
  # running out; a closed input waits it out. In easy mode, a close button
  # in its top-right corner (menu.sh), over the blank row above the text.
  printf '\033[?1000h\033[?1006h'
  . "${0%/*}/menu.sh"
  menu_close_on
  menu_close_draw
  read -rsn 1 -t "$SAY_SECONDS" || sleep 0
  exit 0
fi

# Only a box this script opened on that client: each names its client in its
# command line, so another client's popup is never closed for it.
last_client() { tmux list-clients -F '#{client_activity} #{client_name}' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-; }

# wait: the same box read to its end, a key or its time running out, for
# what has to be seen before what comes next takes the screen.
if [ "${1:-}" = close ] || [ "${1:-}" = wait ]; then
  set -- "$1" "${2:-$(last_client)}"
  # One still opening is waited for, a second at most.
  marker=$(pending "$2")
  for _ in 1 2 3 4 5 6 7 8 9 10; do [ -e "$marker" ] || break; sleep 0.1; done
  rm -f "$marker"
  box="bash $(cd "$(dirname "$0")" && pwd)/$(basename "$0") box $2"
  if [ "$1" = wait ]; then
    while ps -axo command= 2>/dev/null | grep -qxF -- "$box"; do sleep 0.2; done
  else
    ps -axo command= 2>/dev/null | grep -qxF -- "$box" && tmux display-popup -C ${2:+-c "$2"} 2>/dev/null
  fi
  exit 0
fi

client="" seconds=5 after=""
while [ $# -gt 1 ]; do
  case $1 in
    -c) client=$2; shift 2 ;;
    -t) seconds=$2; shift 2 ;;
    -a) after=1; shift ;;
    *) break ;;
  esac
done
text=${1:-}
[ -n "$text" ] || exit 0
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

# Said from inside a popup: tmux takes a second popup asked for while one is
# open as a change to that one, so the server says it a moment later, once
# the popup this runs in has closed.
if [ -n "$after" ]; then
  # run-shell expands formats in its command before the shell reads it, so
  # every # is doubled after the shell quoting, the text reaching say.sh as
  # it is.
  command=$(printf '%q ' "$self" ${client:+-c "$client"} -t "$seconds" "$text")
  tmux run-shell -b "sleep 0.4; ${command//#/##}"
  exit 0
fi

if [ "$(tmux show -gqv @setting_message_boxes 2>/dev/null)" = off ]; then
  # display-message reads its text as a format, so every # is doubled.
  tmux display-message ${client:+-c "$client"} "${text//\#/##}"
  exit 0
fi

# Asked for no client, the one used last, as tmux would pick, named so the
# box can be closed for it.
[ -n "$client" ] || client=$(last_client)

# As wide as the text, wrapped to at most 72 columns, and as tall as it.
wrapped=$(printf '%s\n' "$text" | fold -s -w 72)
width=$(printf '%s\n' "$wrapped" | awk '{ if (length > n) n = length } END { print n + 6 }')
[ "$width" -lt 30 ] && width=30
height=$(( $(printf '%s\n' "$wrapped" | wc -l) + 3 ))
mkdir -p "${XDG_STATE_HOME:-$HOME/.local/state}/switchboard" && : > "$(pending "$client")"
# In the background: a binding or hook that says something never waits on it.
# The text goes in the environment (-e), as it is, never in the command line.
tmux display-popup ${client:+-c "$client"} -E -w "$width" -h "$height" \
  -e "SAY_TEXT=$wrapped" -e "SAY_SECONDS=$seconds" "bash $(printf %q "$self") box $(printf %q "$client")" >/dev/null 2>&1 &
exit 0
