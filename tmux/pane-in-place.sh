#!/usr/bin/env bash
# A tool in a pane's place, leaving the rest of the window, sidebar included,
# as it is; quitting the tool puts the pane back. M-p's lazygit opens this way,
# M-d diffview.nvim (nvim/lua/plugins/diffview.lua), in the stripped-down
# Neovim nvim/diff.lua sets up, and M-p's PR description or Linear ticket as
# an image (tmux/pr-image.js), whose pane alone lets images through to the
# terminal.
#
# Usage:
#   pane-in-place.sh open <lazygit|diffview|pr-image|ticket-image> <pane> <dir> <client>
#                                                           start the tool in <dir> in <pane>'s place
#   pane-in-place.sh back <pane>                            from the tool's pane as it exits: swap <pane> back
#   pane-in-place.sh popup <lazygit|diffview|pr-image|ticket-image> <dir> <client> the tool in <dir> in a popup on <client>, for a
#                                                           worktree no tab is in (the sidebar's d and L)
#   pane-in-place.sh side <pane> <client>                   the pane the tool stands in for, back beside it on its left
#   pane-in-place.sh full <pane> <client>                   the tool alone again, that pane parked once more
#     <pane> is either one: the tool's, or the pane beside it
#
# The tool starts in a window of its own, and swap-pane trades places: the
# tool goes where the pane was, and the pane waits in that window, marked
# @pane_parked so the sidebar gives it no sidebar and leaves it out of the
# tabs. The tool's pane records the parked pane in @parked_pane and its own
# name in @in_place, which the lazygit binding and M-d use to tell their tool's pane, since
# its command reads as the shell that runs the tool. When the tool exits,
# however it is quit, its shell swaps the pane back and selects it, and the
# parked window, left with only the tool's closing pane, closes with it. A
# pane killed instead (M-w's confirmation) is swapped back by confirm.sh
# first, from @parked_pane.

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${BASH_SOURCE[0]%/*}/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }
set -u

# The command that runs <tool> in <dir>, or nothing. <client> is shown a
# popup while it waits on the network. <pane>, when there is one, names the
# tab whose name may carry a ticket.
tool_command() {
  local tool=$1 dir=$2 client=$3 pane=${4:-} base ref ticket
  case $tool in
    lazygit) echo lazygit ;;
    # Under an interactive zsh, for the PATH (node, gh) the tmux server lacks.
    pr-image) echo "zsh -i -c 'node ~/.config/tmux/pr-image.js'" ;;
    # The ticket the branch names, else the tab's name (a "Shaping ABC-123: …"
    # tab before there is a branch), as M-i finds it (ticket-view.sh).
    ticket-image)
      ticket=$(git -C "$dir" branch --show-current 2>/dev/null | grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
      [ -n "$ticket" ] || [ -z "$pane" ] ||
        ticket=$(tmux display -p -t "$pane" '#{@tab_name}' | grep -oE '[A-Z][A-Z0-9]*-[0-9]+' | head -1)
      # It goes into a command line, so only a ticket ID's characters.
      if ! [[ $ticket =~ ^[A-Z][A-Z0-9]*-[0-9]+$ ]]; then
        "${0%/*}/say.sh" -c "$client" "No Linear ticket in this branch or tab name"
        return
      fi
      echo "zsh -i -c 'node ~/.config/tmux/pr-image.js --ticket $ticket'"
      ;;
    # GitHub's "Files changed": the branch's commits since it left its PR's
    # base branch, which diff-base.sh works out and fetches (shared with the
    # story told ahead of time, diff-story-ahead.sh). Committed changes only,
    # as on GitHub. gh and the fetch each wait on the network, about a
    # second, so a popup says it is loading until they are done. It is
    # started in the background, since display-popup holds until the popup
    # closes, and closes itself after 30 seconds should nothing close it.
    # Both are skipped while the branch is at the commit last compared.
    diffview)
      if ! git -C "$dir" rev-parse --git-dir >/dev/null 2>&1; then
        "${0%/*}/say.sh" "Not in a git repository: $dir"
        exit 0
      fi
      if ! ref=$("${0%/*}/diff-base.sh" cached "$dir"); then
        tmux display-popup -c "$client" -E -w 60 -h 9 -T " diffview " \
          "printf '\\n\\n\\n%14s\\033[1mFetching the PR base branch...\\033[0m' ''; sleep 30" </dev/null >/dev/null 2>&1 &
        ref=$("${0%/*}/diff-base.sh" fetch "$dir")
        fetched=$?
        tmux display-popup -C -c "$client"
        [ "$fetched" -ne 3 ] || "${0%/*}/say.sh" -c "$client" "Could not fetch ${ref}; comparing with it as last fetched."
      fi
      echo "nvim -u ~/.config/nvim/diff.lua --noplugin --cmd 'let g:diff_pane = 1' --cmd 'let g:diff_base = \"$ref\"' -c 'DiffviewOpen $ref...HEAD'"
      ;;
  esac
}

case "${1:-}" in
  open)
    tool=$2 pane=$3 dir=$4 client=$5
    command=$(tool_command "$tool" "$dir" "$client" "$pane")
    [ -n "$command" ] || exit 0
    command="$command; ~/.config/tmux/pane-in-place.sh back $pane"
    tool_pane=$(tmux new-window -d -P -F '#{pane_id}' -c "$dir" "$command") || exit 0
    tmux set -w -t "$tool_pane" @pane_parked 1
    tmux set -p -t "$tool_pane" @parked_pane "$pane"
    tmux set -p -t "$tool_pane" @in_place "$tool"
    # The image view draws through tmux's passthrough, for its pane alone.
    case $tool in pr-image | ticket-image) tmux set -p -t "$tool_pane" allow-passthrough on ;; esac
    tmux swap-pane -d -s "$tool_pane" -t "$pane"
    tmux select-pane -t "$tool_pane"
    # How to get the pane back, the first time (tmux/tip.sh).
    ~/.config/tmux/tip.sh show in-place "$client"
    ;;
  popup)
    tool=$2 dir=$3 client=$4
    command=$(tool_command "$tool" "$dir" "$client")
    [ -n "$command" ] || exit 0
    # A box said on the way (a fetch that failed) is read first: a key, or
    # its few seconds, and then the tool.
    "${0%/*}/say.sh" wait "$client"
    tmux display-popup -c "$client" -E -w 95% -h 90% -d "$dir" -T " $tool " "$command"
    ;;
  # Side by side: the parked pane (a Claude session, say) joins the tool's
  # window on its left, half the width, and goes back to waiting in a window
  # of its own on full. Quitting the tool either way brings the pane back
  # whole, as before (back's swap-pane works within one window too).
  side | full)
    current=$2 client=${3:-}
    # The tool's pane: this one, or the one in this window standing in for it.
    if [ -n "$(tmux show -pqv -t "$current" @in_place)" ]; then
      tool_pane=$current
    else
      tool_pane=$(LC_ALL=en_US.UTF-8 tmux list-panes -t "$current" -F $'#{pane_id}\t#{@parked_pane}' | awk -F'\t' -v p="$current" '$2 == p { print $1; exit }')
    fi
    parked=${tool_pane:+$(tmux show -pqv -t "$tool_pane" @parked_pane)}
    if [ -z "$parked" ] || ! tmux display -p -t "$parked" '' 2>/dev/null; then
      "${0%/*}/say.sh" ${client:+-c "$client"} "No tool here standing in for another pane (lazygit, the diff, a PR or ticket image)"
      exit 0
    fi
    together=$([ "$(tmux display -p -t "$parked" '#{window_id}')" = "$(tmux display -p -t "$tool_pane" '#{window_id}')" ] && echo 1)
    if [ "$1" = side ]; then
      [ -n "$together" ] && exit 0
      tmux join-pane -d -h -b -l 50% -s "$parked" -t "$tool_pane"
    else
      [ -n "$together" ] || exit 0
      tmux break-pane -d -s "$parked"
      tmux set -w -t "$parked" @pane_parked 1
      tmux select-pane -t "$tool_pane"
    fi
    ;;

  back)
    pane=$2
    # The pane may have been closed meanwhile; then there is nothing to bring back.
    tmux swap-pane -d -s "$pane" -t "${TMUX_PANE:-}" 2>/dev/null && tmux select-pane -t "$pane"
    ;;
esac
exit 0
