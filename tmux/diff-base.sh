#!/usr/bin/env bash
# What M-d's diff compares a branch with: its PR's base branch (origin's
# default branch without a PR, main where origin doesn't say), as origin has
# it now, so it is fetched first: a base last fetched before the branch was
# rebased onto a newer one would count that newer work as the branch's.
# Shared by M-d (pane-in-place.sh) and the diff story told ahead of time
# (diff-story-ahead.sh), so both compare with the same base and so tell the
# same story, which Neovim keeps by the diff's own text.
#
# Usage:
#   diff-base.sh cached <dir>   the base kept for the branch's commit, when it
#                               is still there; exits 4 with none (nothing
#                               asked of the network)
#   diff-base.sh fetch <dir>    asks GitHub for the PR's base and fetches it,
#                               keeping the answer for the branch's commit;
#                               exits 3 when the fetch failed, the base then
#                               as last fetched
#
# Prints the ref to compare with (origin/<base>, or <base> in a repository
# with no origin). Exits 1 outside a git repository.
#
# The cache holds, per worktree, the commit last compared and its base: the
# merge base can't move while the branch stays at that commit, so neither
# gh nor the fetch is needed again until it moves.

set -u
mode=${1:-} dir=${2:-$PWD}
here=${0%/*}
. "$here/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }

facts=$(git -C "$dir" rev-parse --absolute-git-dir HEAD 2>/dev/null) || exit 1
head=${facts##*$'\n'}
cache="${XDG_CACHE_HOME:-$HOME/.cache}/switchboard/diff-base/$(printf '%s' "${facts%%$'\n'*}" | tr / %)"

if [ "$mode" = cached ]; then
  cached_head= ref=
  IFS=$'\t' read -r cached_head ref 2>/dev/null <"$cache"
  if [ "$cached_head" = "$head" ] && [[ $ref =~ ^[A-Za-z0-9._/-]+$ ]] && git -C "$dir" rev-parse --verify --quiet "$ref" >/dev/null; then
    printf '%s\n' "$ref"
    exit 0
  fi
  exit 4
fi

count_request GitHub GraphQL 'M-d diff: PR base' gh
base=$(cd "$dir" && gh pr view --json baseRefName -q .baseRefName 2>/dev/null)
[ -n "$base" ] || base=$(git -C "$dir" rev-parse --abbrev-ref origin/HEAD 2>/dev/null)
base=${base#origin/}
[ -n "$base" ] || base=main
# It goes into a command line, so only a branch name's characters.
[[ $base =~ ^[A-Za-z0-9._/-]+$ ]] || base=main
fetched=1
count_git_request "$dir" 'M-d diff: fetch base'
git -C "$dir" fetch --quiet origin "$base" 2>/dev/null || fetched=
# A repository with no origin compares with its own branch.
ref=origin/$base
git -C "$dir" rev-parse --verify --quiet "$ref" >/dev/null || ref=$base
# Only a comparison with a freshly fetched base is worth keeping.
[ -z "$fetched" ] || { mkdir -p "${cache%/*}" && printf '%s\t%s\n' "$head" "$ref" >"$cache"; }
printf '%s\n' "$ref"
[ -n "$fetched" ] || exit 3
exit 0
