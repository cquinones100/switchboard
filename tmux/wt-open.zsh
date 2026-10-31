#!/bin/zsh

# Popup for the M-W binding: asks for tickets and starts work on each in
# its own new tab, the way /wt does from inside a Claude session and wt-batch
# does from a shell. Takes one ticket or a list, as wt-batch does.
#
# Usage: wt-open.zsh
#
# The new tabs go at the end of the tab list, in order, start in the focused
# pane's directory (the binding's -d) so `wt` finds that repository, and the
# first is selected. `wt` is typed into each tab's interactive shell rather than
# run as its command, as wt-batch does: the shell is what reads ~/.zshrc, where
# `wt` is defined and node is put on the PATH, and it stays in the worktree
# after Claude exits. That is also why this is zsh and not a call to wt-batch:
# popups run with the tmux server's environment, which has no node.
#
# The field starts with the clipboard when it holds ticket IDs or issue
# URLs. zsh for vared, which edits a prefilled line.

dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'

# Esc cancels; KEYTIMEOUT=1 keeps it from waiting for an escape sequence.
KEYTIMEOUT=1
bindkey -e
bindkey '\e' send-break

fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

ticket_id='^[A-Za-z][A-Za-z0-9]*-[0-9]+$'
issue_url='/(issue|browse)/([A-Za-z][A-Za-z0-9]*-[0-9]+)'

# One ticket, as an ID or a Linear or Jira issue URL, becomes ABC-123 in $ticket.
# Fails on anything else: the ID is typed into a shell below, so nothing but an
# ID may get through.
parse_ticket() {
  local token=$1
  [[ $token =~ $issue_url ]] && token=$match[2]
  [[ $token =~ $ticket_id ]] || return 1
  ticket=${(U)token}
}

# Commas, spaces, or both separate tickets, the way wt-batch splits them.
tokens_of() { print -r -- ${(s:,:)${1//[[:space:]]/,}}; }

# The clipboard fills the field when every piece of it is a ticket, so a copied
# list works as well as a single link.
clipboard=$(pbpaste 2>/dev/null | head -1)
input=
if [[ -n ${clipboard//[[:space:],]/} ]]; then
  input=$clipboard
  for token in $(tokens_of "$clipboard"); do
    parse_ticket "$token" || { input=; break; }
  done
fi

print
print "  ${dim}Ticket IDs or URLs, separated by commas or spaces.${reset}"
print "  ${dim}Enter starts wt for each in a new tab. Esc cancels.${reset}"
print
vared -p '  Tickets: ' input || exit 0

# Every ticket is checked before any tab opens, so a typo in one leaves none
# half-started, and a ticket given twice opens once.
tickets=()
for token in $(tokens_of "$input"); do
  parse_ticket "$token" || fail "Not a ticket ID or ticket URL: $token"
  (( ${tickets[(Ie)$ticket]} )) || tickets+=($ticket)
done
(( ${#tickets} )) || exit 0

git rev-parse --git-dir >/dev/null 2>&1 ||
  fail "This tab is not in a repository, so wt has nowhere to make the worktrees. Run it from the repo the tickets belong to."

# Each tab goes at the end, so the order matches the input and no open tab is
# renumbered: M-1 to M-9 reach tabs by number. -d keeps focus while they
# open, and the first is selected at the end.
first=
for ticket in $tickets; do
  window=$(tmux new-window -d -c "$PWD" -n "$ticket" -P -F '#{window_id}') ||
    fail "tmux could not open a tab for $ticket."
  tmux send-keys -t "$window" "wt $ticket" Enter
  first=${first:-$window}
done
tmux select-window -t "$first"
