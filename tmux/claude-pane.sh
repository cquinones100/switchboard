# Sourced by agent-state.sh, tab-status.sh and pr-ready.sh, which need to know
# whether a pane runs Claude Code. bin/slack-relay asks the same thing in
# lib/slack-relay/tabs.js, so a change here belongs there too.
#
# tmux reports Claude Code's executable by its name, which is its version
# (2.1.285). Started from a Node script, as claude-here starts it, Claude runs
# as the script's child in the terminal's foreground and tmux reports node.

# Whether a pane's command can be Claude, from the name alone: no process.
maybe_claude() { [[ $1 =~ ^[0-9]+\.[0-9]+\.[0-9]+$ || $1 == node ]]; }

# claude_pid <command> <tty>: prints the Claude Code process in the pane's
# foreground and succeeds, or fails. It counts only a foreground process Claude
# Code itself registered: it writes ~/.claude/sessions/<pid>.json for each of
# its processes and nothing else writes there, so a program that happens to be
# named like a version, or any other node program, isn't taken for Claude.
# Checked this way rather than with lsof on the executable, which is as
# certain but costs a third of a second a pane and saturates the CPU when
# wt-jump asks for every window at once.
claude_pid() {
  local pid
  maybe_claude "$1" || return 1
  for pid in $(ps -t "${2#/dev/}" -o pid=,stat= | awk '$2 ~ /\+/ {print $1}'); do
    if [ -f "$HOME/.claude/sessions/$pid.json" ]; then
      echo "$pid"
      return 0
    fi
  done
  return 1
}

# claude_type <pane> <text>: types the text into Claude Code's prompt. With
# Claude Code's vim mode, the prompt can sit in Normal mode (its mode line, at
# the screen's foot, reading -- NORMAL --), where the first letters typed are
# taken as vim commands until one starts Insert mode, so `i` goes first there.
# typeIntoClaude in lib/sidebar/tmux.js does the same for the sidebar daemon.
claude_type() {
  if tmux capture-pane -p -t "$1" | awk 'NF' | tail -n 3 | grep -q -- '-- NORMAL --'; then
    tmux send-keys -t "$1" i
  fi
  tmux send-keys -t "$1" -l "$2"
}
