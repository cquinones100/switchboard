#!/bin/zsh

# Popup for the M-p palette's "Open a ticket in the browser": asks for a
# ticket, in Linear or Jira as the tracker setting says, and opens it in the
# browser. The ticket's counterpart to pr-open.zsh.
#
# Usage: ticket-open.zsh
#
# Takes a ticket ID (ABC-123), a Linear or Jira URL, or a bare number, which
# gets the team key of the ticket the focused pane's branch names. Ctrl-T in
# the field opens that branch's own ticket instead, what M-l does. When the
# clipboard already holds a ticket ID or URL, the field starts with it, since
# that is usually where it came from.
#
# The URL comes from tracker.sh, from Linear's workspace or Jira's site in
# ~/.config/switchboard (linear.json, jira.json), as linear-ticket.sh finds it, so
# no workspace, site or team name lives in this repository.
#
# zsh for vared, which edits a prefilled line.

dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'

# Esc cancels; KEYTIMEOUT=1 keeps it from waiting for an escape sequence.
KEYTIMEOUT=1
bindkey -e
bindkey '\e' send-break

this_tab=false
open-this-tab() { this_tab=true; zle accept-line; }
zle -N open-this-tab
bindkey '^T' open-this-tab

# Errors stay up until a key is pressed; a popup that closed on them would
# look like nothing happened.
fail() {
  print "\n  ${red}$1${reset}"
  print -n "\n  ${dim}Press any key to close.${reset}"
  read -sk 1
  exit 0
}

ticket_id='^[A-Za-z][A-Za-z0-9]*-[0-9]+$'
ticket_url='^https://(linear\.app/[^/]+/issue|[A-Za-z0-9-]+\.atlassian\.net/browse)/[A-Za-z][A-Za-z0-9]*-[0-9]+([/?#].*)?$'
bare_number='^#?[0-9]+$'

# The ticket the focused pane's branch names, if any: the same match as
# linear-ticket.sh.
branch_ticket=$(git branch --show-current 2>/dev/null | grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')

clipboard=$(pbpaste 2>/dev/null | head -1 | tr -d '[:space:]')
input=
[[ $clipboard =~ $ticket_id || $clipboard =~ $ticket_url ]] && input=$clipboard

print
print "  ${dim}A ticket ID, number or URL, then Enter.   Ctrl-T: this tab's ticket   Esc: cancel${reset}"
print
vared -p '  Ticket: ' input || exit 0
input=${input//[[:space:]]/}

if $this_tab; then
  [[ -n $branch_ticket ]] || fail "This tab's branch doesn't name a ticket."
  input=$branch_ticket
fi
[[ -z $input ]] && exit 0

# A URL opens as it is; anything else needs the workspace or site.
if [[ $input =~ $ticket_url ]]; then
  open "$input"
  exit 0
fi

if [[ $input =~ $bare_number ]]; then
  [[ -n $branch_ticket ]] ||
    fail "Ticket ${input#\#} needs a team key, and this tab's branch doesn't name one. Type it in full, like ABC-${input#\#}."
  input="${branch_ticket%%-*}-${input#\#}"
elif [[ ! $input =~ $ticket_id ]]; then
  fail "Not a ticket ID, number or ticket URL: $input"
fi

url=$(~/.config/tmux/tracker.sh url "${input:u}" 2>&1) || fail "$url"

print -n "  ${dim}Opening ${input:u}...${reset}"
open "$url"
