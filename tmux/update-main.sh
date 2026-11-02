#!/usr/bin/env bash
# Popup for the M-p palette's "Update local main to origin's": runs
# bin/update-main for the focused pane's repository (the binding's -d) and
# keeps the popup open until a key, so what it did can be read. Through an
# interactive zsh, for the PATH (node, ~/.local/bin) the tmux server lacks.
#
# Usage: update-main.sh   (in the pane's directory)
#
# The fetch can take a while, so a spinner says it is working until
# update-main answers, and its answer then takes the spinner's line.

out=$(mktemp) || exit 1
trap 'rm -f "$out"' EXIT
zsh -i -c 'update-main "$1"' update-main "$PWD" </dev/null >"$out" 2>&1 &
job=$!
frames=(⠋ ⠙ ⠹ ⠸ ⠼ ⠴ ⠦ ⠧ ⠇ ⠏)
i=0
printf '\n\033[?25l'
while kill -0 "$job" 2>/dev/null; do
  printf '\r  %s Updating main from origin…' "${frames[i++ % ${#frames[@]}]}"
  sleep 0.1
done
wait "$job"
printf '\r\033[K\033[?25h'
sed 's/^/  /' "$out"
printf '\n  \033[38;5;244mPress any key to close.\033[0m'
read -rsn 1
