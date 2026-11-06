#!/usr/bin/env bash
# Opens the Linear ticket named in the current branch.
# Usage: linear-ticket.sh <pane_current_path>
#
# The workspace slug comes from $LINEAR_WORKSPACE, or the linearWorkspace key in
# ~/.config/switchboard/linear.json — the same config worktree-from-ticket
# reads. Keeping it there means no workspace or team identifier lives in this repo.

cd "$1" 2>/dev/null || exit 0

# Matches any team key, so this carries no hardcoded prefix. A branch like
# fix-2-thing can match spuriously; that just opens a URL Linear 404s on.
ticket=$(git branch --show-current 2>/dev/null |
  grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
[ -z "$ticket" ] && exit 0

config="$HOME/.config/switchboard/linear.json"
workspace="${LINEAR_WORKSPACE:-$(jq -r '.linearWorkspace // empty' "$config" 2>/dev/null)}"
[ -z "$workspace" ] && exit 0

"${BASH_SOURCE[0]%/*}/open-url.sh" "https://linear.app/$workspace/issue/$ticket"
