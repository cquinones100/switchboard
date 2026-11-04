#!/usr/bin/env bash
# M-p's "Remind me about this tab": a popup asking when, as Slack's "Remind me
# about this" does (in 20 minutes, in an hour, in 3 hours, tomorrow, next
# week, or a time typed in: 45m, 2h, 1d, 3pm, 15:30), then what for, which
# may be left empty. The tab goes under the sidebar's Reminders with the time
# left, and when it comes due it moves to Needs you with a banner, until you
# go to the tab (agent-state.sh seen). With a reminder set, "Clear the
# reminder" comes first. The time and note are the window's @remind_at
# (seconds) and @remind_note; the sidebar daemon draws them (reminders
# setting).
#
# Usage: remind.sh <window id>
#   remind.sh at <text> [<now>]   print the time <text> names, in seconds (for the tests)

set -u
here=${0%/*}

# The time a choice or typed text names, in seconds, or nothing.
when() {
  local text now hour minute meridiem n target
  text=$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]' | tr -d ' ')
  now=${2:-$(date +%s)}
  case $text in
    "in20minutes") echo $((now + 20 * 60)); return ;;
    "in1hour") echo $((now + 3600)); return ;;
    "in3hours") echo $((now + 3 * 3600)); return ;;
    "tomorrowat9:00") date -j -r "$now" -v+1d -v9H -v0M -v0S +%s; return ;;
    "nextmondayat9:00") date -j -r "$now" -v+1d -v+mon -v9H -v0M -v0S +%s; return ;;
  esac
  if [[ $text =~ ^([0-9]+)(m|min|mins|minutes?)$ ]]; then echo $((now + 10#${BASH_REMATCH[1]} * 60))
  elif [[ $text =~ ^([0-9]+)(h|hr|hrs|hours?)$ ]]; then echo $((now + 10#${BASH_REMATCH[1]} * 3600))
  elif [[ $text =~ ^([0-9]+)(d|days?)$ ]]; then echo $((now + 10#${BASH_REMATCH[1]} * 86400))
  elif [[ $text =~ ^([0-9]{1,2})(:([0-9]{2}))?(am|pm)?$ ]]; then
    hour=$((10#${BASH_REMATCH[1]})) minute=$((10#${BASH_REMATCH[3]:-0})) meridiem=${BASH_REMATCH[4]}
    [ -z "$meridiem" ] && [ -z "${BASH_REMATCH[2]}" ] && return
    # With am or pm, an hour of 1 to 12 only.
    [ -n "$meridiem" ] && { [ "$hour" -lt 1 ] || [ "$hour" -gt 12 ]; } && return
    [ "$meridiem" = pm ] && [ "$hour" -lt 12 ] && hour=$((hour + 12))
    [ "$meridiem" = am ] && [ "$hour" -eq 12 ] && hour=0
    [ "$hour" -gt 23 ] || [ "$minute" -gt 59 ] && return
    target=$(date -j -r "$now" -v"${hour}"H -v"${minute}"M -v0S +%s)
    # A time already past today is tomorrow's, by the calendar, so a change
    # of clocks overnight leaves it at the time asked for.
    [ "$target" -le "$now" ] && target=$(date -j -r "$now" -v+1d -v"${hour}"H -v"${minute}"M -v0S +%s)
    echo "$target"
  fi
}

if [ "${1:-}" = at ]; then
  when "${2:-}" "${3:-}"
  exit 0
fi

window=${1:-}
[ -n "$window" ] || exit 0
if [ "$(tmux show -gqv @setting_reminders)" = off ]; then
  "$here/say.sh" -a "Reminders are off in Settings."
  exit 0
fi
name=$(tmux display -p -t "$window" '#{window_index} #{?@tab_label,#{@tab_label},#{window_name}}')
set_at=$(tmux show -wqv -t "$window" @remind_at)

choices="In 20 minutes
In 1 hour
In 3 hours
Tomorrow at 9:00
Next Monday at 9:00"
[ -n "$set_at" ] && choices="Clear the reminder
$choices"
# Enter on a choice takes it; on text that matches none, the text itself.
picked=$(printf '%s\n' "$choices" | fzf --print-query --bind 'enter:accept-or-print-query' --no-info --layout reverse --height 100% \
  --prompt '  When? ' --header "Remind me about ${name}. Or type a time: 45m, 2h, 1d, 3pm, 15:30. Esc cancels.")
[ $? -eq 130 ] && exit 0
# What was typed wins when it names a time, since fzf matches it to a
# choice it only resembles (2m to "In 20 minutes"); else the choice.
typed=$(printf '%s\n' "$picked" | sed -n 1p)
choice=$(printf '%s\n' "$picked" | sed -n 2p)
[ -n "$typed" ] && [ -n "$(when "$typed")" ] && choice=$typed
[ -n "$choice" ] || choice=$typed
[ -n "$choice" ] || exit 0

if [ "$choice" = "Clear the reminder" ]; then
  tmux set -wu -t "$window" @remind_at \; set -wu -t "$window" @remind_note \; set -wu -t "$window" @remind_said
  "$here/sidebar.sh" nudge 2>/dev/null
  "$here/say.sh" -a "Reminder cleared for ${name}."
  exit 0
fi
at=$(when "$choice")
if [ -z "$at" ]; then
  "$here/say.sh" -a "\"$choice\" isn't a time I know. Try 45m, 2h, 1d, 3pm or 15:30."
  exit 0
fi

note=$(fzf --print-query --no-info --layout reverse --height 100% --prompt '  What for? ' \
  --header "Optional. Enter sets the reminder; Esc cancels." </dev/null)
[ $? -eq 130 ] && exit 0
note=$(printf '%s\n' "$note" | sed -n 1p)

tmux set -w -t "$window" @remind_at "$at" \; set -w -t "$window" @remind_note "$note"
"$here/sidebar.sh" nudge 2>/dev/null
"$here/say.sh" -a "Reminder set for ${name}, $(date -j -r "$at" '+%a %-I:%M %p')."
