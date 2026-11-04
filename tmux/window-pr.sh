#!/usr/bin/env bash

# Prints the pull request number for the worktree at <path>, or nothing.
#
# Usage: window-pr.sh <path>
#
# This is called for every window on every pane focus, so it never waits on the
# network. It answers from a cache and refreshes that cache in the background
# when it has gone stale, which means a PR opened a moment ago shows up on the
# next focus rather than this one. That is the trade that keeps switching
# windows instant; asking gh inline would cost a second per window, every time.

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${BASH_SOURCE[0]%/*}/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }
set -u
# What it keeps names your branches, tickets and prompts: yours alone.
umask 077

path="${1:-}"
[ -n "$path" ] && [ -d "$path" ] || exit 0

script_dir="$(dirname "$0")"
cache_dir="${XDG_CACHE_HOME:-$HOME/.cache}/tmux-window-pr"

# How long an answer is trusted, in minutes. A branch keeps its PR number for
# good, so a found one is worth a day. A branch with no PR yet is usually a
# branch whose agent is about to open one, so that answer goes stale fast.
FOUND_TTL=1440
MISSING_TTL=1

# wtpr bakes the number into the directory name, so that case is already
# answered — no cache, no gh, no background anything.
dir_name=$(basename "$path")
# Only a number, though: pr-12abc isn't one, and is looked up like any other.
case "$dir_name" in
  pr-[0-9]*)
    number=${dir_name#pr-}
    number=${number%%-*}
    if [[ $number =~ ^[0-9]+$ ]]; then
      printf '%s' "$number"
      exit 0
    fi
    ;;
esac

branch=$(git -C "$path" --no-optional-locks rev-parse --abbrev-ref HEAD 2>/dev/null) || exit 0
[ -n "$branch" ] && [ "$branch" != HEAD ] || exit 0

# Keyed on the repository rather than the worktree, so the ticket worktree and
# a wtpr worktree on the same branch share one answer.
common=$(git -C "$path" --no-optional-locks rev-parse --git-common-dir 2>/dev/null) || exit 0
repo=$(cd "$path" && realpath "$common" 2>/dev/null) || exit 0
key=$(printf '%s\n%s' "$repo" "$branch" | shasum -a 256 | cut -c1-16)
cache="$cache_dir/$key"
lock="$cache.lock"

cached=""
[ -f "$cache" ] && cached=$(cat "$cache" 2>/dev/null)

# find -mmin rather than stat, whose flags differ between BSD and GNU.
fresh=false
if [ -f "$cache" ]; then
  if [ -n "$cached" ]; then ttl=$FOUND_TTL; else ttl=$MISSING_TTL; fi
  [ -n "$(find "$cache" -mmin "-$ttl" 2>/dev/null)" ] && fresh=true
fi

# Whatever is known goes out now, even when it is about to be rechecked.
printf '%s' "$cached"

$fresh && exit 0

# A refresh that died before releasing its lock must not wedge the branch
# forever, so a lock nobody has touched in two minutes is abandoned.
[ -d "$lock" ] && [ -n "$(find "$lock" -maxdepth 0 -mmin +2 2>/dev/null)" ] &&
  rmdir "$lock" 2>/dev/null

mkdir -p "$cache_dir"

# mkdir is the atomic part: whichever window gets there first does the lookup
# and the other five return immediately.
mkdir "$lock" 2>/dev/null || exit 0

(
  trap 'rmdir "$lock" 2>/dev/null' EXIT

  # --state all so a merged PR keeps its number in the tab; the work in that
  # window is the same work it was before it merged.
  # A fork's PR (wtpr) is on a branch named <owner>-<branch> that tracks the
  # fork's, which only gh pr view finds, from that upstream.
  merge=$(git -C "$path" config "branch.$branch.merge" 2>/dev/null)
  if [ -n "$merge" ] && [ "${merge#refs/heads/}" != "$branch" ]; then
    count_request GitHub GraphQL 'tab name: PR' gh
    found=$(cd "$path" && gh pr view --json number --jq .number 2>/dev/null)
  else
    count_request GitHub GraphQL 'tab name: PR' gh
    found=$(cd "$path" && gh pr list --head "$branch" --state all --limit 1 \
      --json number --jq '.[0].number' 2>/dev/null)
  fi

  printf '%s' "$found" > "$cache.$$" && mv "$cache.$$" "$cache"

  # Only when the answer actually changed, so the common case — still no PR —
  # renames nothing. The next focus would pick this up anyway; this is what
  # makes a number appear while you are sitting in the window watching it.
  # path-facts.sh's answers carry the old number, so they go first.
  if [ "$found" != "$cached" ]; then
    rm -f "${XDG_CACHE_HOME:-$HOME/.cache}/tmux-path-facts/"*
    tmux list-sessions -F '#{session_name}' 2>/dev/null | while IFS= read -r session; do
      "$script_dir/window-names.sh" "$session"
    done
  fi
) >/dev/null 2>&1 &
