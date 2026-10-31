#!/usr/bin/env bash
# The tab's ticket, from Linear or Jira as the tracker setting says, rendered
# in a pane to the right, as pr-view.sh shows its PR: the facts from the
# ticket's side panel, the description (glow for the
# Markdown, mermaid-ascii for Mermaid diagrams, through pr-view.js), sub-issues,
# related tickets, links, and the comments with their replies. M-i opens it and
# closes it again; q in it closes it too. M-l still opens the ticket in the
# browser.
#
# Usage:
#   ticket-view.sh toggle <pane>   M-i: open the view beside <pane>, or close it
#   ticket-view.sh show <ID>       in the view's pane: render ticket <ID>, the copy
#                                  last read at once, then read again
#
# The ticket is the one the pane's branch names, as linear-ticket.sh finds it,
# else the one the tab's name gives (a "Shaping ABC-123: ..." tab, before
# there is a branch). The tracker is asked through tracker.sh, with the key
# worktree-from-ticket uses. The view's pane carries @ticket_view, so the key
# finds it to close it, and runs under an interactive zsh for the PATH (glow,
# node, ~/.local/bin) the tmux server lacks.

set -u

case "${1:-}" in
  toggle)
    here=${2:-}
    [ -n "$here" ] || exit 0
    open=$(tmux list-panes -t "$here" -F '#{pane_id} #{@ticket_view}' | awk '$2 == "1" { print $1; exit }')
    if [ -n "$open" ]; then
      tmux kill-pane -t "$open"
      exit 0
    fi
    dir=$(tmux display -p -t "$here" '#{pane_current_path}')
    ticket=$(git -C "$dir" branch --show-current 2>/dev/null | grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')
    [ -n "$ticket" ] ||
      ticket=$(tmux display -p -t "$here" '#{@tab_name}' | grep -oE '[A-Z][A-Z0-9]*-[0-9]+' | head -1)
    # It goes into a command line, so only a ticket ID's characters.
    if ! [[ $ticket =~ ^[A-Z][A-Z0-9]*-[0-9]+$ ]]; then
      tmux display-message "No ticket in this branch or tab name"
      exit 0
    fi
    pane=$(tmux split-window -h -l 45% -t "$here" -c "${dir//\#/##}" -P -F '#{pane_id}' "zsh -i -c '~/.config/tmux/ticket-view.sh show $ticket'") || exit 0
    tmux set-option -p -t "$pane" @ticket_view 1
    ;;

  show)
    id=${2:-}
    dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'
    wait_key() {
      printf '\n  %s\n\n  %sPress any key to close.%s' "$1" "$dim" "$reset"
      read -rsn 1
      exit 0
    }
    command -v glow >/dev/null || wait_key "${red}glow isn't installed.${reset} Install it with: brew install glow"
    work=$(mktemp -d)
    refresh=""
    trap '[ -z "$refresh" ] || kill "$refresh" 2>/dev/null; rm -rf "$work"' EXIT
    # From Linear or Jira, as the tracker setting says (tracker.sh), in
    # Linear's shape either way. What went wrong is left in $why.
    fetch() { why=$("${0%/*}/tracker.sh" show "$id" 2>&1 >"$1"); }
    # The ticket as last read is drawn at once and read again in the
    # background, drawn again if it changed, as pr-view.sh does a PR. The
    # tracker is part of the cache's name, so switching it never shows the
    # other one's ticket.
    tracker=$(tmux show -gqv @setting_tracker 2>/dev/null)
    cache="${XDG_CACHE_HOME:-$HOME/.cache}/switchboard/ticket-view/${tracker:-linear}-$id.json"
    if [ -s "$cache" ]; then
      cp "$cache" "$work/ticket.json"
    else
      printf '\n  %sReading %s...%s' "$dim" "$id" "$reset"
      fetch "$work/ticket.json" || wait_key "${red}${why:-No answer about $id.}${reset}"
      mkdir -p "${cache%/*}" && cp "$work/ticket.json" "$cache"
      fetched=1
    fi
    # Laid out and paged as the PR view is, drawn again when the pane is resized.
    trap '' INT
    "${0%/*}/pr-view.sh" page ticket "$work/ticket.json" "$work" </dev/tty &
    pager=$!
    if [ -z "${fetched:-}" ]; then
      {
        fetch "$work/fresh.json" && ! cmp -s "$work/fresh.json" "$cache" || exit 0
        cp "$work/fresh.json" "$cache" && mv "$work/fresh.json" "$work/ticket.json" && kill -WINCH "$pager" 2>/dev/null
      } &
      refresh=$!
    fi
    while kill -0 "$pager" 2>/dev/null; do wait "$pager"; done
    ;;
esac
exit 0
