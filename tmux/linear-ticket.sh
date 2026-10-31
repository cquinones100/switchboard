#!/usr/bin/env bash
# Opens the ticket named in the current branch, in Linear or Jira as the
# tracker setting says. (Named from when Linear was the only one.)
# Usage: linear-ticket.sh <pane_current_path>
#
# The URL comes from tracker.sh, from Linear's workspace ($LINEAR_WORKSPACE or
# linearWorkspace) or Jira's site ($JIRA_SITE or jiraSite) in
# ~/.config/switchboard/linear.json or jira.json — the same config worktree-from-ticket
# reads. Keeping it there means no workspace, site or team identifier lives in this repo.

cd "$1" 2>/dev/null || exit 0

# Matches any team or project key, so this carries no hardcoded prefix. A
# branch like fix-2-thing can match spuriously; that just opens a URL the
# tracker 404s on.
ticket=$(git branch --show-current 2>/dev/null |
  grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
[ -z "$ticket" ] && exit 0

url=$("${0%/*}/tracker.sh" url "$ticket" 2>&1) || { tmux display-message "${url//\#/##}"; exit 0; }
open "$url"
