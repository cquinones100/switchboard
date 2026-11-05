#!/usr/bin/env bash

# Sends the last reply Claude gave in this tab to another tab, for the M-p
# action: pick the tab, and the reply is pasted into its Claude's prompt (or
# its active pane when it runs no Claude) without Enter, and that tab comes to
# the front, so you can add to it before sending.
#
# Usage: reply-send.sh   (from a popup: the tab is the one it was opened over)
#
# The reply is read by bin/claude-reply, a node script. The tmux server has no
# node on its PATH (see sidebar.sh's start), so nvm's default is added here.
#
# It goes in as a bracketed paste, so its line breaks stay in the text rather
# than each pressing Enter.

set -u
# Its lists look like mission control's boxes (fzf-style.sh).
export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${BASH_SOURCE[0]%/*}/fzf-style.sh" 2>/dev/null)"
export LC_ALL=en_US.UTF-8

if ! command -v node >/dev/null 2>&1; then
  # nvm installs into ~/.nvm, or under XDG_CONFIG_HOME when that was set.
  for NVM_DIR in ${NVM_DIR:+"$NVM_DIR"} "$HOME/.nvm" "${XDG_CONFIG_HOME:-$HOME/.config}/nvm"; do
    [ -s "$NVM_DIR/nvm.sh" ] && break
  done
  default=$(cat "$NVM_DIR/alias/default" 2>/dev/null)
  candidate="$NVM_DIR/versions/node/v${default#v}/bin"
  if [ -n "$default" ] && [ -x "$candidate/node" ]; then
    export PATH="$candidate:$PATH"
  elif [ -s "$NVM_DIR/nvm.sh" ]; then
    . "$NVM_DIR/nvm.sh" >/dev/null 2>&1
  fi
fi

# A popup that closes the instant something goes wrong reads as a flicker.
fail() {
  printf '\n  %s\n\n  Press any key to close.' "$1"
  read -rsn 1
  exit 0
}

reply_tool=~/switchboard/bin/claude-reply
source=$(tmux display-message -p '#{window_id}')
reply_file=$(mktemp -t reply-send)
trap 'rm -f "$reply_file"' EXIT
error=$(node "$reply_tool" "$source" 2>&1 >"$reply_file") || fail "$error"

# Every other tab, by its number and name.
target=$(
  tmux list-windows -a -F $'#{window_id}\t#{window_index}  #{?@tab_label,#{@tab_label},#{window_name}}' |
    awk -F'\t' -v me="$source" '$1 != me' |
    fzf --delimiter=$'\t' --with-nth=2 --layout=reverse --height=100% \
      --prompt='send to> ' \
      --header="$(head -c 300 "$reply_file" | tr '\n' ' ' | cut -c1-110)…" |
    cut -f1
)
[ -n "$target" ] || exit 0

pane=$(node "$reply_tool" --pane "$target" 2>&1) || fail "$pane"
tmux load-buffer -b reply-send "$reply_file" &&
  tmux paste-buffer -p -d -b reply-send -t "$pane" &&
  tmux select-window -t "$target" &&
  tmux select-pane -t "$pane"
