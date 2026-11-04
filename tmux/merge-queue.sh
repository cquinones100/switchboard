#!/usr/bin/env bash
# Opens the repository's merge queue in the browser, from the M-p palette's popup.
# Usage: merge-queue.sh [path]   (defaults to the current directory, which is
# how the popup passes the pane's)
#
# A queue belongs to a branch, so this opens the one the branch's PR targets,
# and the default branch's when there is no PR. A repository without a merge
# queue shows GitHub's empty queue page.
#
# One gh call either way: the PR's URL holds the repository's, and without a
# PR, gh repo view gives the repository's URL and default branch together.
# The popup shows that it is working meanwhile, and closes once the browser
# has the page; when there is nothing to open, it says why and waits for a key.

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${BASH_SOURCE[0]%/*}/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }
dim=$'\033[2m' reset=$'\033[0m'
say() {
  printf '\r\033[2K  %s\n\n  %sPress any key to close.%s' "$1" "$dim" "$reset"
  read -rsn 1
  exit 0
}

cd "${1:-$PWD}" 2>/dev/null || say "No such directory: ${1:-$PWD}"
printf '\n  %sFinding the merge queue...%s' "$dim" "$reset"

count_request GitHub GraphQL 'open the merge queue' gh
if pr=$(gh pr view --json url,baseRefName --jq '"\(.url)\t\(.baseRefName)"' 2>/dev/null); then
  IFS=$'\t' read -r pr_url base <<<"$pr"
  url=${pr_url%/pull/*}
else
  count_request GitHub GraphQL 'open the merge queue' gh
  repo=$(gh repo view --json url,defaultBranchRef --jq '"\(.url)\t\(.defaultBranchRef.name)"' 2>/dev/null) ||
    say "This isn't a GitHub repository gh can reach."
  IFS=$'\t' read -r url base <<<"$repo"
fi
[ -n "$url" ] && [ -n "$base" ] || say "GitHub didn't say which branch's queue to open."

open "$url/queue/$base"
