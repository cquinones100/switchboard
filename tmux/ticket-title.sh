#!/usr/bin/env bash

# Prints the title of ticket <ID>, from Linear or Jira as the tracker setting
# says, or nothing when it is not known yet or there is no such ticket.
#
# Usage: ticket-title.sh <ID>
#
# Same rule as window-pr.sh: answer from a cache, never wait on the network,
# refresh in the background when the answer has gone stale. The key is the one
# worktree-from-ticket uses (lib/tracker).

set -u
# What it keeps names your branches, tickets and prompts: yours alone.
umask 077

id="${1:-}"
[ -n "$id" ] || exit 0

script_dir="$(dirname "$0")"
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/tmux-ticket-title"
cache="$cache_dir/$id"
lock="$cache.lock"

# A title rarely changes, so a found one is good for a day. A miss is usually a
# branch name that only looks like a ticket, so it is rechecked less eagerly
# than window-pr.sh rechecks a missing PR.
FOUND_TTL=1440
MISSING_TTL=60

cached=""
[ -f "$cache" ] && cached=$(cat "$cache" 2>/dev/null)

fresh=false
if [ -f "$cache" ]; then
  if [ -n "$cached" ]; then ttl=$FOUND_TTL; else ttl=$MISSING_TTL; fi
  [ -n "$(find "$cache" -mmin "-$ttl" 2>/dev/null)" ] && fresh=true
fi

printf '%s' "$cached"

$fresh && exit 0

[ -d "$lock" ] && [ -n "$(find "$lock" -maxdepth 0 -mmin +2 2>/dev/null)" ] &&
  rmdir "$lock" 2>/dev/null

mkdir -p "$cache_dir"
mkdir "$lock" 2>/dev/null || exit 0

(
  trap 'rmdir "$lock" 2>/dev/null' EXIT

  # Asked of the tracker the tracker setting names (tracker.sh). Only a
  # definite answer is cached: a title, or 3 for no such ticket. A network or
  # auth failure leaves the old answer in place, so a blip never blanks a
  # title that was already known.
  title=$("$script_dir/tracker.sh" title "$id")
  case $? in
    0) ;;
    3) title="" ;;
    *) exit 0 ;;
  esac

  printf '%s' "$title" > "$cache.$$" && mv "$cache.$$" "$cache"

  # Same as window-pr.sh: only when the answer changed, so a title shows up in
  # the tab while you are sitting in the window rather than on the next focus.
  # path-facts.sh's answers carry the old title, so they go first.
  if [ "$title" != "$cached" ]; then
    rm -f "${XDG_CACHE_HOME:-$HOME/.cache}/tmux-path-facts/"*
    tmux list-sessions -F '#{session_name}' 2>/dev/null | while IFS= read -r session; do
      "$script_dir/window-names.sh" "$session"
    done
  fi
) >/dev/null 2>&1 &
