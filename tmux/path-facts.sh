#!/usr/bin/env bash

# Prints, tab-separated, what tmux shows for the directory <path> that only
# changes with its branch: the accent color, the label it would have with no
# name set (the ticket and its title, else accent-color.sh --name), and
# the PR number, if any.
#
# Usage: path-facts.sh <path>
#
# tab-status.sh asks on every status redraw and window-names.sh for every
# window on every pane focus, and working all three out takes a dozen git,
# shasum and cache lookups. With a security agent inspecting every process
# that starts, that made tmux sluggish. So the answer is kept per directory
# while the branch is the one it was worked out on, which costs one git call to
# check: for FOUND_SECONDS once it has a PR, whose number never changes, and
# for MISSING_SECONDS without one, about as often as window-pr.sh rechecks a
# branch with no PR. When window-pr.sh or ticket-title.sh learn
# something new in the background, they clear these answers before renaming
# the tabs, so a PR number or title still shows up as soon as it is known.

set -u
# What it keeps names your branches, tickets and prompts: yours alone.
umask 077

path="${1:-}"
[ -n "$path" ] || exit 0

script_dir="${0%/*}"
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/tmux-path-facts"
cache="$cache_dir/${path//\//%}"
FOUND_SECONDS=600
MISSING_SECONDS=60

branch=$(git -C "$path" --no-optional-locks rev-parse --abbrev-ref HEAD 2>/dev/null)
now=$(date +%s)

if [ -f "$cache" ]; then
  { IFS=$'\t' read -r at cached_branch; IFS= read -r facts; } <"$cache"
  ttl=$MISSING_SECONDS
  [[ ${facts-} == *$'\t'*$'\t'?* ]] && ttl=$FOUND_SECONDS
  if [ "${cached_branch-}" = "$branch" ] && [[ ${at-} =~ ^[0-9]+$ ]] && (( now - at < ttl )); then
    printf '%s' "$facts"
    exit 0
  fi
fi

hex=$("$script_dir/accent-color.sh" "$path")

# Matches any team or project key, the same way linear-ticket.sh does. A
# branch that only looks like a ticket is harmless: the tracker has no such
# ticket, ticket-title.sh prints nothing, and the directory name is used as usual.
label=""
# With the tab_lookups setting off (tmux/settings.sh), neither the ticket
# tracker nor GitHub is asked: the tab takes its directory's name and no PR.
# The tracker and github settings turn off each one's half.
IFS='|' read -r lookups tracker github < <(tmux display -p '#{@setting_tab_lookups}|#{@setting_tracker}|#{@setting_github}' 2>/dev/null)
ticket=$(printf '%s' "$branch" | grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
if [ -n "$ticket" ] && [ "${lookups:-}" != off ] && [ "${tracker:-}" != off ]; then
  title=$("$script_dir/ticket-title.sh" "$ticket")
  [ -n "$title" ] && label="$ticket: $title"
fi
[ -n "$label" ] || label=$("$script_dir/accent-color.sh" --name "$path")
# Tabs separate the fields, so none may be in one.
label=${label//$'\t'/ }

pr=""
[ "${lookups:-}" = off ] || [ "${github:-}" = off ] || pr=$("$script_dir/window-pr.sh" "$path")

facts="$hex"$'\t'"$label"$'\t'"$pr"
mkdir -p "$cache_dir"
{ printf '%s\t%s\n%s\n' "$now" "$branch" "$facts" >"$cache.$$" && mv "$cache.$$" "$cache"; } 2>/dev/null
printf '%s' "$facts"
