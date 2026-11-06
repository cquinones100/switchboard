#!/usr/bin/env bash
# What it keeps (context use, model) is yours alone.
umask 077

input=$(cat)
model=$(echo "$input" | jq -r '.model.display_name')
raw_cwd=$(echo "$input" | jq -r '.workspace.current_dir')
cwd=$(echo "$raw_cwd" | sed "s|^$HOME|~|")
used=$(echo "$input" | jq -r '.context_window.used_percentage // empty')

# The session's context use and model, for the sidebar's Sessions view
# (lib/sidebar/sessions.js), which has no other way to know them: one small
# file per session, written only when it changes.
session=$(echo "$input" | jq -r '.session_id // empty')
if [[ $session =~ ^[A-Za-z0-9-]+$ && -n $used ]]; then
  context_dir=~/.local/state/tmux-sidebar/context
  line=$(printf '%s\t%s' "$used" "$model")
  if [ "$(cat "$context_dir/$session" 2>/dev/null)" != "$line" ]; then
    mkdir -p "$context_dir" && printf '%s' "$line" >"$context_dir/$session"
  fi
fi

# Spend and rate limits, for the Sessions view's usage rows, which the
# statusline alone is told: spend/<session> is "<day>\t<spent that day>\t<the
# session's cost so far>\t<the Claude process's pid>", the day's share kept
# across days and restarts, and limits is the account's rate_limits, the same
# for every session. Within one process the cost the statusline is given dips
# and recovers, so a lower cost from the same pid adds nothing and keeps the
# higher one; only a new pid, from the session file naming this session,
# counts from nothing. A record from before the pid was kept may hold a share
# counted many times over, so today's is cut to the session's cost so far,
# and since it can't tell whether this draw comes from the process that wrote
# it, a lower cost adds nothing and becomes the cost to count on from.
cost=$(echo "$input" | jq -r '.cost.total_cost_usd // empty')
if [[ $session =~ ^[A-Za-z0-9-]+$ && $cost =~ ^[0-9.]+$ ]]; then
  usage_dir=~/.local/state/tmux-sidebar/usage
  spend_file=$usage_dir/spend-$session
  old=$(cat "$spend_file" 2>/dev/null)
  session_file=$(grep -lF "\"sessionId\":\"$session\"" ~/.claude/sessions/*.json 2>/dev/null | head -1)
  pid=${session_file##*/}
  pid=${pid%.json}
  new=$(printf '%s' "$old" | awk -F'\t' -v today="$(date +%F)" -v cost="$cost" -v pid="$pid" '
    { day = $1; spent = $2; last = $3; was = $4; fields = NF }
    END {
      if (day != today) spent = 0
      else if (fields == 3 && spent > last) spent = last
      if (pid == "") pid = was
      delta = cost - last
      if (was != "" && pid != was) delta = cost
      else if (delta < 0) { delta = 0; if (fields != 3) cost = last }
      printf "%s\t%.4f\t%s\t%s", today, spent + delta, cost, pid
    }')
  if [ "$new" != "$old" ]; then
    mkdir -p "$usage_dir" && printf '%s' "$new" >"$spend_file"
  fi
  limits=$(echo "$input" | jq -c '.rate_limits // empty')
  if [ -n "$limits" ] && [ "$(cat "$usage_dir/limits" 2>/dev/null)" != "$limits" ]; then
    printf '%s' "$limits" >"$usage_dir/limits"
  fi
fi

branch=$(git -C "$raw_cwd" --no-optional-locks rev-parse --abbrev-ref HEAD 2>/dev/null)

# Derive a deterministic accent color from the directory + git context.
# Same HSL logic (S=80%, L=45%) and color key (dirname + branch/worktree) as
# tmux/accent-color.sh.
dirname=$(basename "$raw_cwd")
color_key="$dirname"
if [ -n "$branch" ]; then
  git_common_dir=$(git -C "$raw_cwd" --no-optional-locks rev-parse --git-common-dir 2>/dev/null)
  git_dir=$(git -C "$raw_cwd" --no-optional-locks rev-parse --git-dir 2>/dev/null)
  if [ -n "$git_common_dir" ] && [ -n "$git_dir" ]; then
    real_common=$(cd "$raw_cwd" && realpath "$git_common_dir" 2>/dev/null)
    real_git=$(cd "$raw_cwd" && realpath "$git_dir" 2>/dev/null)
    if [ "$real_git" != "$real_common" ]; then
      worktree_name=$(basename "$real_git")
      color_key="${dirname} | worktree:${worktree_name}"
    else
      color_key="${dirname} | branch:${branch}"
    fi
  fi
fi
hash=$(printf '%s' "$color_key" | shasum -a 256 | cut -c1-8)
hue=$(( 16#$hash % 360 ))
h_segment=$(( hue / 60 ))
h_frac=$(( (hue % 60) * 255 / 60 ))
c=204
x=$(( c * (255 - (h_frac * 2 - 255 > 0 ? h_frac * 2 - 255 : 255 - h_frac * 2)) / 255 ))
m=35
case $h_segment in
  0) r=$((c+m)); g=$((x+m)); b=$m ;;
  1) r=$((x+m)); g=$((c+m)); b=$m ;;
  2) r=$m; g=$((c+m)); b=$((x+m)) ;;
  3) r=$m; g=$((x+m)); b=$((c+m)) ;;
  4) r=$((x+m)); g=$m; b=$((c+m)) ;;
  *) r=$((c+m)); g=$m; b=$((x+m)) ;;
esac
(( r > 255 )) && r=255; (( g > 255 )) && g=255; (( b > 255 )) && b=255
(( r < 0 )) && r=0; (( g < 0 )) && g=0; (( b < 0 )) && b=0
accent=$'\033[38;2;'"${r};${g};${b}"m
reset=$'\033[0m'

# Context usage color gradient (independent of accent)
ctx_color=""
if [ -n "$used" ]; then
  used_int=$(printf "%.0f" "$used")
  if [ "$used_int" -lt 20 ]; then
    ctx_color=$'\033[97m'
  elif [ "$used_int" -lt 40 ]; then
    ctx_color=$'\033[32m'
  elif [ "$used_int" -lt 60 ]; then
    ctx_color=$'\033[38;5;178m'
  elif [ "$used_int" -lt 80 ]; then
    ctx_color=$'\033[38;5;208m'
  else
    ctx_color=$'\033[38;5;160m'
  fi
fi

# Build output with accent-colored model and directory. Inside tmux the tab and
# the second status line already name the directory and branch, so only the
# model and context are shown there; in a plain terminal this is the only place
# they appear.
output="${accent}${model}${reset}"

if [ -z "${TMUX:-}" ]; then
  output="${output} | ${accent}${cwd}${reset}"
  [ -n "$branch" ] && output="${output} | ${accent}${branch}${reset}"
fi

if [ -n "$used" ]; then
  printf '%s | Context: %s%.1f%% used%s' "$output" "$ctx_color" "$used" "$reset"
else
  printf '%s' "$output"
fi
