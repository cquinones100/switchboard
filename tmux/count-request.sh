#!/bin/bash
# The request audit for shell scripts (lib/requests.js is Node's): sourced,
# count_request <service> <protocol> <what made it> [<command>] appends a
# line to today's file, as lib/requests.js does, with builtins only; with a
# command named (gh, curl), only when that command is there to run, since one
# that isn't makes no request. Never fails its caller.

count_request() {
  local dir=$HOME/.local/state/switchboard/requests now day
  # Sourced by zsh too (pr-open.zsh): a glob matching nothing is empty there, for this function alone.
  [ -z "${ZSH_VERSION:-}" ] || setopt localoptions nullglob
  [ -z "${4:-}" ] || command -v "$4" >/dev/null 2>&1 || return 0
  # A host on this Mac is no service outside it, as in lib/requests.js.
  case $1 in localhost | 127.* | ::1 | "[::1]" | 0.0.0.0) return 0 ;; esac
  printf -v now '%(%s)T' -1 2>/dev/null || now=$(date +%s)
  printf -v day '%(%Y-%m-%d)T' -1 2>/dev/null || day=$(date +%Y-%m-%d)
  [ -d "$dir" ] || mkdir -p "$dir" 2>/dev/null || return 0
  # Once a day, whoever wrote the day's first line, the days past the two
  # weeks kept go, as lib/requests.js drops them, so shell-only use ages them
  # out too (.pruned holds the day it last ran).
  local last=
  { read -r last <"$dir/.pruned"; } 2>/dev/null
  if [ "$last" != "$day" ]; then
    # By the day each file is named for, as lib/requests.js compares them.
    local cutoff file name
    cutoff=$(date -v-14d +%Y-%m-%d 2>/dev/null || date -d '14 days ago' +%Y-%m-%d 2>/dev/null)
    if [ -n "$cutoff" ]; then
      for file in "$dir"/[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9].tsv; do
        [ -e "$file" ] || continue
        name=${file##*/}
        [[ ${name%.tsv} < $cutoff ]] && rm -f "$file"
      done
    fi
    printf '%s\n' "$day" >"$dir/.pruned" 2>/dev/null
  fi
  printf '%s\t%s\t%s\t%s\n' "$now" "$1" "$2" "${3:-}" >>"$dir/$day.tsv" 2>/dev/null
  return 0
}

# A git fetch or pull from <dir>'s remote (origin by default): counted under
# the host it goes to (GitHub for github.com), and not at all for a remote on
# this Mac (a path or file://).
count_git_request() {
  local url host
  url=$(git -C "$1" remote get-url "${3:-origin}" 2>/dev/null) || return 0
  # A URL with a scheme (not file://), or git's scp form (host:path, no /
  # before the colon); anything else is a path on this Mac.
  if [[ $url == file://* ]]; then
    return 0
  elif [[ $url =~ ^[A-Za-z][A-Za-z0-9+.-]*:// ]]; then
    host=${url#*://}
  elif [[ $url =~ ^[^/]+: ]]; then
    host=$url
  else
    return 0
  fi
  host=${host#*@}
  host=${host%%[/:]*}
  case $host in github.com | gist.github.com) host=GitHub ;; esac
  count_request "$host" git "$2" git
}
