#!/usr/bin/env bash

# Tracks what each Claude session in tmux is doing, so the tab bar can show
# which ones are waiting on you, which have finished, and which are working.
#
# Usage:
#   agent-state.sh set <working|waiting|done|clear>   from Claude Code hooks, for $TMUX_PANE; reads their JSON
#   agent-state.sh notification               from the Notification hook; reads its JSON
#   agent-state.sh seen <window_id>           from the pane-focus-in hook
#   agent-state.sh interrupted <pane> <time>  from the sidebar daemon: a turn interrupted on a wait
#   agent-state.sh next                       from M-a: go to the tab most in need of you
#   agent-state.sh idle-next                  from M-A: go to the next idle tab
#
# Each pane running Claude carries @agent_state (working, waiting, or done) and
# @agent_since (when it got there). Its window carries @agent, the most urgent
# state among its panes (waiting, then done, then working), which the
# @agent_marker format in tmux.conf turns into the mark in the tab.
#
# A pane also carries what Claude is doing, from the hook's JSON, for the
# sidebar (lib/sidebar) to show: @agent_activity, its tool call in a few
# words ("Run the test suite", "editing render.js"), set as each tool starts
# (PreToolUse) and kept until the next, and @agent_wait, what it waits on
# ("permission: Bash git push", "asks: Which layout?"), while it waits. An
# AskUserQuestion call is a wait from the moment it starts. @agent_loop names
# the loop a session is running (review, ci, queue or deploy) from the moment its
# skill is asked for, typed as /review-loop, /ci-loop, /queue-and-clean or
# /deploy-test or
# run by the Skill tool, until a new prompt or session: the turns between a
# loop's rounds are the loop's too.
#
# "done" means finished and not yet looked at: a tab still in front a second
# after it was focused loses it (AGENT_SEEN_DELAY, in seconds), so passing over
# it, as the sidebar does while you browse, leaves it, and a session that
# finishes in the tab you are already looking at never gets it.
#
# Claude Code waits on its hooks, so everything here is a few tmux calls and
# nothing else: no network, nothing slow. Hooks must not print either, since
# Claude Code adds some hooks' output to the conversation, and they always exit
# 0 so a tmux hiccup never blocks Claude.

set -u

# tmux rewrites control characters in its output, tab delimiters included,
# without a UTF-8 locale.
export LC_ALL=en_US.UTF-8

# The relay's own Haiku sessions (bin/slack-relay) inherit these hooks; their
# turns are Slack calls, not work anyone is waiting on.
[ -n "${SLACK_RELAY_HELPER:-}" ] && exit 0

# slack-relay wants to know when any Claude session finishes a turn or starts
# waiting on you, including sessions outside tmux, with the transcript that
# says what happened (a finished turn's event carries its final message, from
# the Stop hook, since the transcript can lag behind), and when a session
# takes a prompt, with its text, so it can tell you Claude has your message. The relay keeps an events file, and
# removes it only when stopped on purpose; each such hook appends one line to
# it. Without the file nothing is written. This runs before the tmux check for
# that reason.
RELAY_EVENTS=~/.local/state/slack-relay/events.jsonl
payload=""
relay_event() {
  [ -f "$RELAY_EVENTS" ] || return 0
  jq -c --arg event "$1" --argjson time "$(date +%s)" \
    '{event: $event, time: $time, session_id, transcript_path, cwd}
      + (if .prompt then {prompt: .prompt[0:300]} else {} end)
      + (if .last_assistant_message then {last: .last_assistant_message} else {} end)' <<<"$payload" >>"$RELAY_EVENTS" 2>/dev/null
}
# A headless Claude (entrypoint sdk-cli in its session file), such as one a
# review loop's script starts, inherits the tab's TMUX_PANE and runs these
# hooks too; its turns are not the tab's, so they mark nothing and tell the
# relay nothing. Its session file is found by the session ID in the input.
from_headless() {
  local session
  session=$(jq -r '.session_id // empty' <<<"$payload" 2>/dev/null)
  [ -n "$session" ] || return 1
  jq -e --arg s "$session" 'select(.sessionId == $s and ((.entrypoint // "") | startswith("sdk")))' ~/.claude/sessions/*.json >/dev/null 2>&1
}
case "${1:-}" in
  set | notification)
    payload=$(cat 2>/dev/null)
    from_headless && exit 0
    ;;
esac
case "${1:-}:${2:-}" in
  set:done | set:waiting)
    relay_event "$2"
    ;;
  set:working)
    # UserPromptSubmit and the tool hooks both say "working"; only the first
    # is a prompt being taken. Read either way, for what Claude is doing.
    if [ -f "$RELAY_EVENTS" ]; then
      [ "$(jq -r '.hook_event_name // empty' <<<"$payload" 2>/dev/null)" = UserPromptSubmit ] && relay_event prompt
    fi
    ;;
  notification:*)
    case $(jq -r '.notification_type // empty' <<<"$payload" 2>/dev/null) in
      permission_prompt | elicitation_dialog | elicitation_url_dialog | agent_needs_input) relay_event waiting ;;
    esac
    ;;
esac

# A conversation moved into the background runs in a job with no tmux of its
# own (no TMUX, no TMUX_PANE), while the tab it came from keeps a parked Claude
# whose session names the job (parkedJobId, matching the job's jobId). Its
# hooks are marked on that tab, so the tab shows what the conversation is
# doing rather than staying idle. The job may itself have been parked into
# another, so the links are followed up to the tab. The session files are a
# handful of small JSON files, so the lookup takes a few milliseconds.
parked_pane() {
  local session=$1 job pid parent hops=0 sessions=~/.claude/sessions
  job=$(jq -r --arg s "$session" 'select(.sessionId == $s) | .jobId // empty' "$sessions"/*.json 2>/dev/null | head -1)
  while [ -n "$job" ] && (( hops++ < 5 )); do
    pid=$(jq -r --arg j "$job" 'select(.parkedJobId == $j) | .pid // empty' "$sessions"/*.json 2>/dev/null | head -1)
    [ -n "$pid" ] || return 1
    parent=$(ps -o ppid= -p "$pid" 2>/dev/null | tr -d ' ')
    # The tab's Claude is the pane's process, or a child of its shell.
    pane=$(tmux list-panes -a -F '#{pane_pid} #{pane_id}' 2>/dev/null | awk -v a="$pid" -v b="$parent" '$1 == a || $1 == b { print $2; exit }')
    [ -n "$pane" ] && { echo "$pane"; return 0; }
    # Not in a tab: that Claude may be a job parked in turn.
    job=$(jq -r --arg p "$pid" 'select((.pid | tostring) == $p) | .jobId // empty' "$sessions"/*.json 2>/dev/null | head -1)
  done
  return 1
}
if [ -z "${TMUX_PANE:-}" ]; then
  case "${1:-}" in
    set | notification)
      session=$(jq -r '.session_id // empty' <<<"$payload" 2>/dev/null)
      [ -n "$session" ] && TMUX_PANE=$(parked_pane "$session") || exit 0
      ;;
  esac
fi

[ -n "${TMUX:-}${TMUX_PANE:-}" ] || exit 0

source "${BASH_SOURCE[0]%/*}/claude-pane.sh"

# A pane still running Claude, from its command and terminal. A pane that has
# gone back to a shell keeps no state, so a session that died without its
# SessionEnd hook does not leave a mark behind. Claude Code's own executable is
# taken on its name, without a process; only a node pane, which may be Claude
# under claude-here, is looked at more closely.
runs_claude() {
  [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || claude_pid "$1" "$2" >/dev/null
}

# Recomputes @agent for one window from its panes, dropping the state of any
# pane no longer running Claude.
refresh() {
  local window=$1 best="" rank=0 pane state command tty r
  while IFS=$'\t' read -r pane state command tty; do
    [ -n "$state" ] || continue
    if ! runs_claude "$command" "$tty"; then
      tmux set -pu -t "$pane" @agent_state 2>/dev/null
      tmux set -pu -t "$pane" @agent_since 2>/dev/null
      continue
    fi
    case $state in waiting) r=3 ;; done) r=2 ;; working) r=1 ;; *) r=0 ;; esac
    (( r > rank )) && { rank=$r; best=$state; }
  done < <(tmux list-panes -t "$window" -F $'#{pane_id}\t#{@agent_state}\t#{pane_current_command}\t#{pane_tty}' 2>/dev/null)
  if [ -n "$best" ]; then
    tmux set -w -t "$window" @agent "$best"
  else
    tmux set -wu -t "$window" @agent 2>/dev/null
  fi
}

set_state() {
  local pane=$1 state=$2 window current
  window=$(tmux display -p -t "$pane" '#{window_id}' 2>/dev/null) || return
  # The same state again changes nothing, so @agent_since keeps when it began:
  # a permission prompt sets "waiting" twice (PermissionRequest, then the
  # Notification hook about six seconds later), and every tool call sets
  # "working", and neither should look like a new wait or a new turn.
  current=$(tmux display -p -t "$pane" '#{@agent_state}' 2>/dev/null)
  [ "$current" = "$state" ] && return
  # Finishing in front of you is not news: the tab you are looking at in an
  # attached session does not get "done".
  if [ "$state" = done ] &&
    [ "$(tmux display -p -t "$pane" '#{&&:#{window_active},#{session_attached}}')" = 1 ]; then
    state=clear
  fi
  if [ "$state" = clear ]; then
    tmux set -pu -t "$pane" @agent_state 2>/dev/null
    tmux set -pu -t "$pane" @agent_since 2>/dev/null
  else
    tmux set -p -t "$pane" @agent_state "$state"
    tmux set -p -t "$pane" @agent_since "$(date +%s)"
  fi
  refresh "$window"
  # The first tab to wait on you, or to finish, while you look elsewhere
  # brings its tip on M-a (tip.sh), once: the format starts nothing after it
  # has been shown.
  if [ "$state" = waiting ] || [ "$state" = done ]; then
    tmux if -F -t "$pane" "#{&&:#{!:#{@tip_seen_$state}},#{!:#{&&:#{window_active},#{session_attached}}}}" "run-shell -b '~/.config/tmux/tip.sh show $state'" 2>/dev/null
  fi
}

# The hook's event and tool, and in a few words what the tool call does and,
# for a permission request or a question, what Claude waits on. One jq, its
# fields separated by \x1f so an empty one stays in place.
DETAILS='
  def base: split("/") | last;
  def short(n): if length > n then .[0:n - 1] + "…" else . end;
  (.tool_input // {}) as $in
  | (.tool_name // "") as $tool
  | [ (.hook_event_name // ""), $tool,
      ( if $tool == "" then ""
        elif $tool == "Bash" then ($in.description // ($in.command // "" | split("\n")[0]))
        elif ($tool | IN("Edit", "Write", "MultiEdit", "NotebookEdit")) then "editing " + (($in.file_path // $in.notebook_path // "") | base)
        elif $tool == "Read" then "reading " + (($in.file_path // "") | base)
        elif ($tool | IN("Grep", "Glob")) then "searching " + ($in.pattern // "")
        elif ($tool | IN("WebSearch", "WebFetch")) then "searching the web"
        elif ($tool | IN("Agent", "Task")) then "agent: " + ($in.description // "")
        elif $tool == "Skill" then "/" + ($in.skill // "")
        elif $tool == "TodoWrite" then "planning"
        elif ($tool | startswith("mcp__")) then ($tool | sub("^mcp__(claude_ai_)?"; "") | sub("__"; ": ") | gsub("_"; " "))
        else $tool end | short(60)),
      ( if .hook_event_name == "PermissionRequest" then "permission: " + $tool + (if $in.command then " " + ($in.command | split("\n")[0]) else "" end) | short(80)
        elif $tool == "AskUserQuestion" then "asks: " + ((($in.questions // [])[0].question) // "") | short(80)
        else "" end),
      ( {"review-loop": "review", "ci-loop": "ci", "queue-and-clean": "queue", "deploy-test": "deploy", "preview-test": "deploy"}[
          if .hook_event_name == "UserPromptSubmit" then ((.prompt // "") | capture("^\\s*/(?<s>[a-z-]+)")?.s // "")
          elif $tool == "Skill" then ($in.skill // "")
          else "" end ] // ""),
      # What started a session (startup, resume, clear, compact) or a
      # compaction (manual, auto).
      (.source // .trigger // "") ]
  | map(gsub("[\t\n\u001f]"; " ")) | join("\u001f")'

# Keeps a pane's @agent_activity and @agent_wait in step with its state: a
# new prompt or a finished turn clears both, work clears the wait and records
# the tool call, and a wait records what it is for.
details() {
  local pane=$1 state=$2 event=$3 activity=$4 wait=$5 loop=$6
  # A loop asked for marks the pane; any other prompt, or a new session, ends it.
  if [ -n "$loop" ]; then
    tmux set -p -t "$pane" @agent_loop "$loop"
  elif [ "$event" = UserPromptSubmit ] || [ "$event" = SessionStart ] || [ "$event" = SessionEnd ]; then
    tmux set -pu -t "$pane" @agent_loop 2>/dev/null
  fi
  # A compaction cut short by a new prompt or session leaves nothing to hand back.
  case $event in
    UserPromptSubmit | SessionStart | SessionEnd)
      tmux set -pu -t "$pane" @agent_compact_from 2>/dev/null
      # A loop's step (queue-and-clean's landing, land-lock.js phase) is set
      # again by the loop as it goes on, so a new prompt or session ends it.
      tmux set -pu -t "$pane" @agent_phase 2>/dev/null
      ;;
  esac
  case $state in
    clear | done)
      tmux set -pu -t "$pane" @agent_activity 2>/dev/null
      tmux set -pu -t "$pane" @agent_wait 2>/dev/null
      ;;
    working)
      tmux set -pu -t "$pane" @agent_wait 2>/dev/null
      if [ "$event" = UserPromptSubmit ] || [ "$event" = PostCompact ]; then
        tmux set -pu -t "$pane" @agent_activity 2>/dev/null
      elif [ "$event" = PreCompact ]; then
        tmux set -p -t "$pane" @agent_activity "compacting context"
      elif [ "$event" = PreToolUse ] && [ -n "$activity" ]; then
        tmux set -p -t "$pane" @agent_activity "$activity"
      fi
      ;;
    waiting)
      [ -n "$wait" ] && tmux set -p -t "$pane" @agent_wait "$wait"
      ;;
  esac
}

case "${1:-}" in
  set)
    [ -n "${TMUX_PANE:-}" ] || exit 0
    IFS=$'\x1f' read -r event tool activity wait loop source < <(jq -r "$DETAILS" <<<"${payload:-{\}}" 2>/dev/null)
    state=${2:-clear}
    # Compaction: Claude is at work on it, whatever the tab showed, and a
    # session it restarts (SessionStart from compact) is the same one going
    # on, so it keeps its state and loop. After a /compact you typed, nothing
    # follows, so the tab is idle again. An automatic one hands the tab back
    # as it was before (@agent_compact_from, its state and since): working in
    # the middle of a turn, idle or done when Claude compacted while idle.
    case "${event:-}:${source:-}" in
      SessionStart:compact) exit 0 ;;
      PreCompact:*)
        before=$(tmux display -p -t "$TMUX_PANE" "#{@agent_state} #{@agent_since}" 2>/dev/null)
        if [ "${before%% *}" = working ]; then
          tmux set -pu -t "$TMUX_PANE" @agent_compact_from 2>/dev/null
        else
          tmux set -p -t "$TMUX_PANE" @agent_compact_from "$before"
        fi
        state=working ;;
      PostCompact:*)
        before=$(tmux display -p -t "$TMUX_PANE" "#{@agent_compact_from}" 2>/dev/null)
        tmux set -pu -t "$TMUX_PANE" @agent_compact_from 2>/dev/null
        if [ "$source" = manual ]; then
          state=clear
        elif [ -n "$before" ]; then
          state=${before%% *}
          state=${state:-clear}
          # A look while it compacted has already made it idle (seen).
          if [ "$state" != clear ]; then
            # As it was, since included, so a finished tab keeps its age.
            tmux set -p -t "$TMUX_PANE" @agent_state "$state"
            tmux set -p -t "$TMUX_PANE" @agent_since "${before#* }"
            refresh "$(tmux display -p -t "$TMUX_PANE" "#{window_id}")"
          fi
        else
          state=working
        fi ;;
    esac
    # A question to you is a wait from the moment it is asked.
    [ "${event:-}:${tool:-}" = PreToolUse:AskUserQuestion ] && state=waiting
    set_state "$TMUX_PANE" "$state"
    details "$TMUX_PANE" "$state" "${event:-}" "${activity:-}" "${wait:-}" "${loop:-}"
    # Tabs are named on a focus change, so a turn that ran a command elsewhere
    # (an interactive shell takes the pane's terminal, and its directory with
    # it) can leave the tab named for that place. At the end of a turn, a pane
    # no longer where its tab's name came from (window-names.sh's @tab_path)
    # names the tabs again, in the background.
    if [ "${event:-}" = Stop ]; then
      tmux if -F -t "$TMUX_PANE" '#{!=:#{pane_current_path},#{@tab_path}}' "run-shell -b -t $TMUX_PANE '~/.config/tmux/window-names.sh #{q:session_name}'" 2>/dev/null
    fi
    # The first Claude to start in a worktree brings the tip on its PR's keys
    # (tip.sh), with GitHub on.
    if [ "${event:-}" = SessionStart ]; then
      tmux if -F -t "$TMUX_PANE" '#{&&:#{&&:#{!:#{@tip_seen_worktree}},#{!=:#{@setting_github},off}},#{m:*/.claude/worktrees/*,#{pane_current_path}}}' "run-shell -b '~/.config/tmux/tip.sh show worktree'" 2>/dev/null
      # One Claude to a tab: one starting beside another moves to a tab of its
      # own (one-claude.sh), in the background, with the setting on.
      tmux if -F -t "$TMUX_PANE" '#{!=:#{@setting_one_claude_per_tab},off}' "run-shell -b -t $TMUX_PANE '~/.config/tmux/one-claude.sh #{q:pane_id}'" 2>/dev/null
    fi
    ;;

  notification)
    [ -n "${TMUX_PANE:-}" ] || exit 0
    # Read above, before the tmux check, so the relay hears about it too.
    kind=$(jq -r '.notification_type // empty' <<<"$payload" 2>/dev/null)
    message=$(jq -r '.message // empty' <<<"$payload" 2>/dev/null)
    # Permission prompts and questions are Claude waiting on you. The
    # PermissionRequest hook has usually said so already, since this hook only
    # fires once a prompt has waited about six seconds. The idle reminder comes
    # a minute after a turn ended, which "done" already shows, and the rest
    # (sign-in, quota) only get the banner.
    case $kind in
      permission_prompt | elicitation_dialog | elicitation_url_dialog | agent_needs_input)
        set_state "$TMUX_PANE" waiting ;;
    esac
    # The banner names the tab, so you know where to go. None with the
    # notifications setting off (tmux/settings.sh).
    IFS=$'\t' read -r banner label < <(tmux display -p -t "$TMUX_PANE" $'#{?@setting_notifications,#{@setting_notifications},on}\t#{window_index}:#{window_name}' 2>/dev/null)
    text=${message:-Claude Code needs your attention}
    [ "${banner:-}" = off ] || osascript -e "display notification \"${text//\"/\\\"}\" with title \"Claude Code: ${label//\"/\\\"}\"" >/dev/null 2>&1
    ;;

  seen)
    window=${2:-}
    [ -n "$window" ] || exit 0
    # The pane-focus-in hook runs this in the background, so the wait holds
    # nothing up.
    sleep "${AGENT_SEEN_DELAY:-1}"
    # With the done_clears setting at "prompt", a finished tab keeps its mark
    # until the next prompt, however long you look at it.
    [ "$(tmux display -p -t "$window" '#{&&:#{&&:#{window_active},#{session_attached}},#{!=:#{@setting_done_clears},prompt}}' 2>/dev/null)" = 1 ] &&
    while IFS=$'\t' read -r pane state from; do
      [ "$state" = done ] && tmux set -pu -t "$pane" @agent_state && tmux set -pu -t "$pane" @agent_since
      # A finished tab seen while it compacts comes back idle, not finished.
      [ "${from%% *}" = done ] && tmux set -p -t "$pane" @agent_compact_from " "
    done < <(tmux list-panes -t "$window" -F $'#{pane_id}\t#{@agent_state}\t#{@agent_compact_from}' 2>/dev/null)
    # A reminder come due is done with once you are at its tab (remind.sh).
    IFS=$'\t' read -r front landed at < <(tmux display -p -t "$window" $'#{&&:#{window_active},#{session_attached}}\t#{?@autopilot_left,1,0}\t#{@remind_at}' 2>/dev/null)
    if [ "${front:-}" = 1 ] && [ -n "${at:-}" ] && [ "$at" -le "$(date +%s)" ]; then
      tmux set -wu -t "$window" @remind_at \; set -wu -t "$window" @remind_note \; set -wu -t "$window" @remind_said 2>/dev/null
    fi
    # A tab of the mode working a note, landed and back in the main clone,
    # has been seen, so the sidebar daemon closes it once you leave it
    # (lib/sidebar/autopilot.js).
    [ "${front:-}" = 1 ] && [ "${landed:-}" = 1 ] && tmux set -w -t "$window" @autopilot_seen 1 2>/dev/null
    # Every window, not just this one, so a session that ended without saying
    # so loses its mark the next time any tab is focused.
    tmux list-windows -a -F '#{window_id}' | while read -r w; do refresh "$w"; done
    ;;

  interrupted)
    # A turn you interrupted (a permission request denied, or Esc) runs no
    # hook, so the pane is left waiting on the request. The sidebar daemon
    # reads the interruption from the transcript and runs this with its time,
    # in seconds: the pane goes idle unless it has moved on since (a prompt
    # typed meanwhile, a new request).
    pane=${2:-} at=${3:-}
    [ -n "$pane" ] && [[ $at =~ ^[0-9]+$ ]] || exit 0
    IFS=$'\t' read -r state since window < <(tmux display -p -t "$pane" $'#{@agent_state}\t#{@agent_since}\t#{window_id}' 2>/dev/null)
    [ "${state:-}" = waiting ] && [[ ${since:-} =~ ^[0-9]+$ ]] && [ "$since" -le "$at" ] || exit 0
    tmux set -pu -t "$pane" @agent_state \; set -pu -t "$pane" @agent_since \; set -pu -t "$pane" @agent_wait 2>/dev/null
    refresh "$window"
    ;;

  idle-next)
    # M-A: the tab after this one among the idle ones, wrapping at the end,
    # or the first when this one isn't idle. The sidebar's Idle, as its daemon
    # writes it (lib/sidebar/daemon.js, writeIdle), when the daemon runs, since
    # only it can tell an idle tab from one working in the background or
    # looping; else every window whose Claude pane has no state, in order.
    # Pressed from a sidebar, the cursor stays in a sidebar, as with M-a.
    from_sidebar=$(tmux display -p '#{@sidebar}')
    here=$(tmux display -p '#{window_id}')
    idle_file=~/.local/state/tmux-sidebar/idle
    daemon=$(cat ~/.local/state/tmux-sidebar/daemon.pid 2>/dev/null)
    if [ -n "$daemon" ] && kill -0 "$daemon" 2>/dev/null && [ -f "$idle_file" ]; then
      list=$(cat "$idle_file")
    else
      # A node pane counts only when Claude runs there (claude-pane.sh), so a
      # dev server or the Slack relay is no idle tab.
      # Fields split on \x1f rather than a tab, which read would close up
      # around an empty @agent_state, @sidebar or @pane_parked.
      list=$(tmux list-panes -a -F $'#{window_id}\x1f#{pane_id}\x1f#{session_id}\x1f#{@agent_state}\x1f#{pane_current_command}\x1f#{@sidebar}\x1f#{@pane_parked}\x1f#{pane_tty}' |
        while IFS=$'\x1f' read -r w p s state command sidebar parked tty; do
          [ -z "$state" ] && [ "$sidebar" != 1 ] && [ "$parked" != 1 ] && runs_claude "$command" "$tty" && printf '%s\t%s\t%s\n' "$w" "$p" "$s"
        done | awk -F'\t' '!seen[$1]++')
    fi
    windows=() panes=() sessions=()
    while IFS=$'\t' read -r w p s; do
      [ -n "$w" ] && windows+=("$w") && panes+=("$p") && sessions+=("$s")
    done <<<"$list"
    n=${#windows[@]}
    if [ "$n" -eq 0 ]; then
      "${0%/*}/say.sh" "No idle tabs."
      exit 0
    fi
    at=-1
    for i in "${!windows[@]}"; do [ "${windows[$i]}" = "$here" ] && at=$i; done
    if [ "$at" -ge 0 ] && [ "$n" -eq 1 ]; then
      "${0%/*}/say.sh" "This is the only idle tab."
      exit 0
    fi
    to=$(( (at + 1) % n ))
    tmux select-window -t "${windows[$to]}"
    if [ "$from_sidebar" = 1 ]; then
      "${0%/*}/sidebar.sh" refocus "${windows[$to]}" &
    elif [ "${panes[$to]}" != - ]; then
      tmux select-pane -t "${panes[$to]}"
    fi
    [ "${sessions[$to]}" != "$(tmux display -p '#{session_id}')" ] && tmux switch-client -t "${sessions[$to]}"
    exit 0
    ;;

  next)
    # Pressed from a sidebar, the cursor stays in a sidebar: the next tab's
    # (sidebar.sh refocus), rather than its pane.
    from_sidebar=$(tmux display -p '#{@sidebar}')
    # The sidebar's Needs you, as its daemon writes it (lib/sidebar/daemon.js,
    # writeNeeds), when the daemon runs: pinned tabs first, then waiting and
    # done, and none that only looks done here while it works in the
    # background or loops, which only the daemon can tell from the transcript.
    # Each press goes to the tab after this one there, wrapping at the end; a
    # done tab leaves the list once seen, so pressed again from it, the walk
    # carries on with the tabs that were after it (@agent_walk_rest). From a
    # tab that only finished, or one not in the list, a tab pinned or waiting
    # on you comes first, before the next finished one; the next press from
    # there picks up where the finished ones were left (@agent_walk_after), so
    # the walk still reaches every one.
    needs_file=~/.local/state/tmux-sidebar/needs
    daemon=$(cat ~/.local/state/tmux-sidebar/daemon.pid 2>/dev/null)
    if [ -n "$daemon" ] && kill -0 "$daemon" 2>/dev/null && [ -f "$needs_file" ]; then
      here=$(tmux display -p '#{window_id}')
      # The Notifications window is the list's "notices" entry, last on it; so
      # is the window the walk last went to the notifications from, when they
      # opened in its sidebar rather than a window of their own (notice_list off):
      # the walk moves on from there. When they are all that needs you, M-a
      # always opens them (below).
      window_here=$here
      [ "$(tmux display -p '#{@notices_window}')" = 1 ] && here=notices
      [ "$(tmux show -gqv @agent_walk_window)" = notices ] && [ "$(tmux show -gqv @agent_notices_from)" = "$window_here" ] && here=notices
      windows=() panes=() sessions=() whys=()
      while IFS=$'\t' read -r w p s why; do
        [ -n "$w" ] && windows+=("$w") && panes+=("$p") && sessions+=("$s") && whys+=("${why:-done}")
      done <"$needs_file"
      n=${#windows[@]}
      if [ "$n" -eq 0 ]; then
        "${0%/*}/say.sh" "Nothing needs you."
        exit 0
      fi
      at=-1
      for i in "${!windows[@]}"; do [ "${windows[$i]}" = "$here" ] && at=$i; done
      # The first tab pinned or waiting on you, other than this one.
      urgent=-1
      for i in "${!windows[@]}"; do
        [ "$i" != "$at" ] && [ "$urgent" -lt 0 ] && [ "${whys[$i]}" != done ] && [ "${whys[$i]}" != notices ] && urgent=$i
      done
      # Where the finished ones were left when the walk went to an urgent one,
      # kept whether or not that one is still listed (answered, it leaves).
      after=-1
      if [ "$(tmux show -gqv @agent_walk_window)" = "$here" ]; then
        resume=$(tmux show -gqv @agent_walk_after)
        for i in "${!windows[@]}"; do [ -n "$resume" ] && [ "${windows[$i]}" = "$resume" ] && [ "$i" != "$at" ] && after=$i; done
      fi
      tmux set -gu @agent_walk_after
      # Where the walk would go on from a tab that left the list (seen, it
      # clears): the first of the rest it noted that is still listed.
      rest=-1
      if [ "$at" -lt 0 ] && [ "$(tmux show -gqv @agent_walk_window)" = "$here" ]; then
        for id in $(tmux show -gqv @agent_walk_rest); do
          for i in "${!windows[@]}"; do [ "$rest" -lt 0 ] && [ "${windows[$i]}" = "$id" ] && rest=$i; done
        done
      fi
      if [ "$urgent" -ge 0 ] && { [ "$at" -lt 0 ] || [ "${whys[$at]}" = done ] || [ "${whys[$at]}" = notices ]; }; then
        to=$urgent
        # From a listed finished tab, the one after it; from one no longer
        # listed, the place already kept.
        if [ "$at" -ge 0 ]; then
          tmux set -g @agent_walk_after "${windows[$(( (at + 1) % n ))]}"
        elif [ "$after" -ge 0 ]; then
          tmux set -g @agent_walk_after "${windows[$after]}"
        elif [ "$rest" -ge 0 ] && [ "$rest" != "$urgent" ]; then
          tmux set -g @agent_walk_after "${windows[$rest]}"
        fi
      elif [ "$after" -ge 0 ]; then
        to=$after
      elif [ "$at" -ge 0 ]; then
        # The notifications alone open again whatever was last done with them.
        if [ "$n" -eq 1 ] && [ "${windows[0]}" != notices ]; then
          "${0%/*}/say.sh" "This is the only tab that needs you."
          exit 0
        fi
        to=$(( (at + 1) % n ))
      else
        to=0
        if [ "$(tmux show -gqv @agent_walk_window)" = "$here" ]; then
          for id in $(tmux show -gqv @agent_walk_rest); do
            found=-1
            for i in "${!windows[@]}"; do [ "${windows[$i]}" = "$id" ] && found=$i; done
            if [ "$found" -ge 0 ]; then
              to=$found
              break
            fi
          done
        fi
      fi
      tmux set -g @agent_walk_window "${windows[$to]}"
      tmux set -g @agent_walk_rest "${windows[*]:$((to + 1))}"
      if [ "${windows[$to]}" = notices ]; then
        tmux set -g @agent_notices_from "$window_here"
        # NOTICES_COMMAND: the tests' stand-in for sidebar.sh notices.
        ${NOTICES_COMMAND:-"${0%/*}/sidebar.sh" notices} "$(tmux display -p '#{client_name}')"
        exit 0
      fi
      tmux select-window -t "${windows[$to]}"
      # Back from the notifications shown in this same window's sidebar, its
      # cursor is still on them, so the tab's pane takes the focus instead.
      back_here=$([ "$here" = notices ] && [ "${windows[$to]}" = "$window_here" ] && echo 1)
      if [ "$from_sidebar" = 1 ] && [ -z "$back_here" ]; then
        "${0%/*}/sidebar.sh" refocus "${windows[$to]}" &
      elif [ "${panes[$to]}" != - ]; then
        tmux select-pane -t "${panes[$to]}"
      elif [ -n "$back_here" ]; then
        # A tab with no Claude pane (pinned by hand): its first pane that isn't the sidebar.
        other=$(tmux list-panes -t "${windows[$to]}" -F '#{pane_id} #{@sidebar}' | awk '$2 != 1 { print $1; exit }')
        [ -n "$other" ] && tmux select-pane -t "$other"
      fi
      [ "${sessions[$to]}" != "$(tmux display -p '#{session_id}')" ] && tmux switch-client -t "${sessions[$to]}"
      exit 0
    fi
    # Without the sidebar's daemon, from the panes' own states:
    # Everything that needs you, in order: waiting first, then done, oldest
    # first within each. A node pane counts too, as Claude may be running under
    # claude-here there; one that isn't loses its state when it is next focused.
    # Each press goes to the entry after the current tab and
    # wraps at the end, so pressing it repeatedly walks the whole list. A tab
    # stays waiting after a visit until you answer it, so it has to be a walk:
    # "the most urgent tab" would keep coming back to the same one.
    here=$(tmux display -p '#{window_id}')
    list=$(tmux list-panes -a -F $'#{@agent_state}\t#{@agent_since}\t#{window_id}\t#{pane_id}\t#{session_name}\t#{pane_current_command}' |
      awk -F'\t' '($1 == "waiting" || $1 == "done") && ($6 ~ /^[0-9]+\.[0-9]+\.[0-9]+$/ || $6 == "node") {
        print ($1 == "waiting" ? 0 : 1) "\t" $2 "\t" $3 "\t" $4 "\t" $5 }' |
      sort -t$'\t' -k1,1n -k2,2n)
    if [ -z "$list" ]; then
      "${0%/*}/say.sh" "No Claude session is waiting on you."
      exit 0
    fi
    # Where the walk got to is kept as the last stop's place in the order, not
    # as a tab: a done tab stops being done the moment it is focused, so by the
    # next press it is no longer in the list to count from. Pressed from that
    # same tab, the walk carries on from its place; pressed anywhere else, it
    # starts again from the top. From a finished tab in the walk, a tab still
    # waiting on you comes first, as with the daemon's list above.
    last_window=$(tmux show -gqv @agent_walk_window)
    last_key=$(tmux show -gqv @agent_walk_key)
    target=$(awk -F'\t' -v here="$here" -v last_window="$last_window" -v last_key="$last_key" '
      BEGIN { split(last_key, k, " ") }
      { line[NR] = $0; rank[NR] = $1; since[NR] = $2; win[NR] = $3 }
      END {
        if (here == last_window && k[1] == 1)
          for (i = 1; i <= NR; i++) if (rank[i] == 0 && win[i] != here) { print line[i]; exit }
        if (here == last_window && last_key != "") {
          for (i = 1; i <= NR; i++)
            if (rank[i] > k[1] || (rank[i] == k[1] && since[i] > k[2]) || (rank[i] == k[1] && since[i] == k[2] && win[i] > k[3])) { print line[i]; exit }
          print line[1]; exit
        }
        for (i = 1; i <= NR; i++) if (win[i] != here) { print line[i]; exit }
        print line[1]
      }' <<<"$list")
    if [ "$(cut -f3 <<<"$target")" = "$here" ]; then
      "${0%/*}/say.sh" "This is the only Claude session waiting on you."
      exit 0
    fi
    tmux set -g @agent_walk_window "$(cut -f3 <<<"$target")"
    tmux set -g @agent_walk_key "$(cut -f1-3 <<<"$target" | tr '\t' ' ')"
    IFS=$'\t' read -r _ _ window pane session <<<"$target"
    # select-window first, so switching sessions lands on it rather than on
    # whatever was last focused there. switch-client only when the session
    # changes, since it needs a client and the rest do not.
    if [ "$from_sidebar" = 1 ]; then
      tmux select-window -t "$window"
      "${0%/*}/sidebar.sh" refocus "$window" &
    else
      tmux select-window -t "$window" \; select-pane -t "$pane"
    fi
    [ "$session" != "$(tmux display -p '#{session_name}')" ] && tmux switch-client -t "$session"
    ;;
esac
# bin/tmux-sidebar's sidebars redraw as soon as a state changes, rather than at
# the daemon's next look; a shell builtin, so no process (see tmux/sidebar.sh).
[ -d ~/.local/state/tmux-sidebar ] && echo >>~/.local/state/tmux-sidebar/nudge
exit 0
