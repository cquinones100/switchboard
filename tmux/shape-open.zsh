#!/bin/zsh

# Popup for the M-p palette's "Continue shaping tickets": asks for
# tickets and carries on shaping each in its own new tab, as
# `shape-batch --continue` does from a shell, without /shape first asking
# whether to run a review loop instead. Takes one ticket or a list, as
# wt-open.zsh does, and starts in the focused pane's directory (the binding's
# -d) unless you pick another, which it asks after the tickets (pick-dir.zsh), so
# `shape` finds that repository.
#
# Usage: shape-open.zsh
#
# Each tab's command is an interactive zsh running `shape --continue <ID>`,
# for the PATH the tmux server lacks, with the ID as its own argument rather
# than typed, as shape-batch does; it is left at a shell when Claude exits.
# The new tabs go at the end, in order, and the first is selected.
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
parse_ticket() {
  local token=$1
  [[ $token =~ $issue_url ]] && token=$match[2]
  [[ $token =~ $ticket_id ]] || return 1
  ticket=${(U)token}
}

# Commas, spaces, or both separate tickets.
tokens_of() { print -r -- ${(s:,:)${1//[[:space:]]/,}}; }

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
print "  ${dim}Enter carries on shaping each in a new tab. Esc cancels.${reset}"
print
vared -p '  Tickets: ' input || exit 0

# Every ticket is checked before any tab opens, and one given twice opens once.
tickets=()
for token in $(tokens_of "$input"); do
  parse_ticket "$token" || fail "Not a ticket ID or ticket URL: $token"
  (( ${tickets[(Ie)$ticket]} )) || tickets+=($ticket)
done
(( ${#tickets} )) || exit 0

# Which repository to shape them in, asked as a new tab asks (pick-dir.zsh).
dir=$("${0:A:h}/pick-dir.zsh" "Shape ${#tickets} ticket(s) here: Enter opens a tab for each") || exit 0
git -C "$dir" rev-parse --git-dir >/dev/null 2>&1 ||
  fail "${dir/#$HOME/~} is not in a repository, so shape has no code to read. Pick the repo the tickets belong to."

first=
for ticket in $tickets; do
  window=$(tmux new-window -d -c "${dir//\#/##}" -n "Shaping $ticket" -P -F '#{window_id}' -- zsh -i -c 'shape --continue "$1"; exec zsh -i' shape "$ticket") ||
    fail "tmux could not open a tab for $ticket."
  first=${first:-$window}
done
tmux select-window -t "$first"
