#!/usr/bin/env bash
# Exits 0 when Switchboard's skills, hooks and instructions load as a plugin in
# its tmux panes (this folder, through CLAUDE_CODE_PLUGIN_DIRS), 1 when setup
# writes them into ~/.claude instead. setup.sh and tmux/settings.sh both ask
# it, so the two never disagree.
#
# The claude_plugin setting (tmux/settings.json, on by default) has to be on,
# Claude Code new enough to read the variable, and either you said yes to moving them (~/.local/state/switchboard/claude-plugin
# holding "moved", which setup's question and turning the setting on in
# Settings write) or nothing of the old way is in ~/.claude (a new install).
# An install from before keeps the old way until asked, so the plugin's hooks
# never run beside the ones in ~/.claude/settings.json.
#
# Usage: in-use.sh [--old]   --old: exit 0 when any of the old way is there

repo="$(cd "$(dirname "$0")/../.." && pwd)"
MIN_VERSION=2.1.280
config=$HOME/.config/switchboard
moved=$HOME/.local/state/switchboard/claude-plugin

# Switchboard's hooks in ~/.claude/settings.json, its import in
# ~/.claude/CLAUDE.md, or a skill link in ~/.claude/skills into a clone's
# claude/skills.
old() {
  local link
  grep -qs 'agent-state.sh' ~/.claude/settings.json && return 0
  grep -qsx '@~/switchboard/claude/switchboard.md' ~/.claude/CLAUDE.md && return 0
  for link in ~/.claude/skills/*; do
    [[ -L $link && $(readlink "$link") == */claude/skills/"${link##*/}" && -d $repo/claude/skills/${link##*/} ]] && return 0
  done
  return 1
}

if [[ ${1:-} == --old ]]; then
  old
  exit
fi

setting=$(jq -r --slurpfile mine <(jq -c 'if type == "object" then . else {} end' "${SWITCHBOARD_SETTINGS:-$config/settings.json}" 2>/dev/null || echo '{}') '
  .[] | select(.key == "claude_plugin") | (if ($mine[0].claude_plugin | type) == "boolean" then $mine[0].claude_plugin else .default end)' "$repo/tmux/settings.json" 2>/dev/null)
[[ $setting == true ]] || exit 1
# A Claude Code too old to read CLAUDE_CODE_PLUGIN_DIRS keeps the old way.
# Found where the tmux server, whose PATH may lack it, finds it too.
claude=$(command -v claude || echo ~/.local/bin/claude)
version=$("$claude" --version 2>/dev/null | awk '{ print $1; exit }')
[[ -z $version || $(printf '%s\n' "$MIN_VERSION" "$version" | sort -t. -k1,1n -k2,2n -k3,3n | head -1) == "$MIN_VERSION" ]] || exit 1
[[ $(cat "$moved" 2>/dev/null) == moved ]] && exit 0
! old
