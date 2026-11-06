#!/usr/bin/env bash
# The health light in the status line: nothing while all is well, a red mark
# and what is wrong when something that should be running quietly stopped.
# Run by #() in status-left (tmux.conf and update-colors.sh), every
# status-interval, so it is one tmux call and file checks, no network.
#
# Usage: health.sh
#
# What it watches:
#   saves    tmux-continuum's last save is over SAVE_LIMIT seconds old. It saves
#            from a #() job in status-right, so a status line that stops
#            drawing status-right stops the saves without a word. With the job
#            gone from status-right it says "(autosave off)" and starts
#            continuum-save.sh to put it back, so the light clears itself.
#   sidebar  the sidebar is on but its daemon (bin/tmux-sidebar) isn't running.
#   relay    bin/slack-relay crashed: its events file is still there, which it
#            removes only when stopped on purpose, but its process has gone.
#   connectors  a Claude connector a feature that is on needs (Slack, Google
#            Calendar, Notion, Fathom…) wasn't connected when it last tried, in
#            the last CONNECTOR_LIMIT seconds (lib/connector.js writes
#            connectors-down, a "<service>\t<when>" line each).
#   setup    a script in ~/switchboard's tmux/ or bin/ with no link in
#            ~/.config/tmux or ~/.local/bin, as a commit made in the clone
#            leaves one until ./setup.sh tmux bin runs (.githooks/post-commit
#            runs it, where this clone uses the hooks).
#
# It also starts tmux/repo-update.sh periodic in the background, which checks
# GitHub for new commits to ~/switchboard at most once an hour, since this
# is the one thing tmux runs on a timer. Nothing here waits for it.

set -u

SAVE_LIMIT=300
CONNECTOR_LIMIT=7200

alive() {
  local pid
  pid=$(cut -d' ' -f1 "$1" 2>/dev/null)
  [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null
}

# status-right last, since it may hold a |.
IFS='|' read -r saved sidebar light updates right < <(tmux display -p '#{@continuum-save-last-timestamp}|#{@sidebar_on}|#{@setting_health_light}|#{@setting_update_check}|#{status-right}' 2>/dev/null)

now=$(date +%s)
# The hour is checked here, with builtins, so most runs start nothing more.
read -r checked 2>/dev/null < ~/.local/state/switchboard/update-checked
# Not with the update_check setting at "open" or "off" (tmux/settings.sh).
if [ "${updates:-hourly}" = hourly ] && [ $((now - ${checked:-0})) -ge 3600 ] && [ -x ~/.config/tmux/repo-update.sh ]; then
  ~/.config/tmux/repo-update.sh periodic >/dev/null 2>&1 &
fi

# The light itself, unless the health_light setting is off.
[ "${light:-on}" = off ] && exit 0

problems=()
if [ -n "${saved:-}" ] && [ $((now - saved)) -gt $SAVE_LIMIT ]; then
  if [[ ${right:-} == *continuum_save.sh* ]]; then
    problems+=("saves $(((now - saved) / 60))m old")
  else
    problems+=("saves $(((now - saved) / 60))m old (autosave off)")
    [ -x ~/.config/tmux/continuum-save.sh ] && { ~/.config/tmux/continuum-save.sh >/dev/null 2>&1 & }
  fi
fi
if [ "${sidebar:-}" != off ] && ! alive ~/.local/state/tmux-sidebar/daemon.pid; then
  problems+=("sidebar down")
fi
if [ -f ~/.local/state/slack-relay/events.jsonl ] && ! alive ~/.local/state/slack-relay/relay.lock; then
  problems+=("relay down")
fi

if [ -s ~/.local/state/tmux-sidebar/connectors-down ]; then
  while IFS=$'\t' read -r service at; do
    [ -n "$service" ] && [ $((now - ${at:-0})) -lt $CONNECTOR_LIMIT ] && problems+=("$service not connected")
  done < ~/.local/state/tmux-sidebar/connectors-down
fi

# Builtin globs and tests only, as this runs on every tick.
unlinked=false
for file in ~/switchboard/tmux/*.sh ~/switchboard/tmux/*.zsh ~/switchboard/tmux/*.js; do
  [ -e "$file" ] && [ ! -e ~/.config/tmux/"${file##*/}" ] && { unlinked=true; break; }
done
if ! $unlinked; then
  for file in ~/switchboard/bin/*; do
    [ -f "$file" ] && [ -x "$file" ] && [ ! -e ~/.local/bin/"${file##*/}" ] && { unlinked=true; break; }
  done
fi
$unlinked && problems+=("setup out of date")

[ ${#problems[@]} -eq 0 ] && exit 0
text=$(printf '%s, ' "${problems[@]}")
printf '#[fg=colour196,bold] ● %s #[default]' "${text%, }"
