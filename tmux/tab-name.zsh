#!/bin/zsh

# Popup for naming a tab, opened by M-p's "Rename the tab", and by new-tab.zsh
# (M-t, M-n) once it has
# opened the window.
# The name goes in the window's @tab_name option, which tab-label.sh puts ahead
# of the ticket and directory in both the tab and the second status line.
#
# Usage: tab-name.zsh [optional]
#   optional: say the name can be skipped, for the prompt after M-t
#
# Three ways to name it, picked with one key:
#   Enter  type a name (the current one is filled in)
#   t      a ticket: "ABC-123: <its title>", as a ticket worktree's tab
#          shows it. In that worktree, no prompt: the name is cleared so the
#          tab shows its own ticket. Elsewhere it asks which ticket.
#   s      shaping a ticket: "Shaping: ABC-123", the form `shape` uses, which
#          /wt clears when it hands the tab over. Like t, it takes the
#          branch's ticket without asking when there is one.
#
# The window is the one the popup is over, asked of tmux rather than passed in:
# display-popup does not expand formats in its command, so a #{window_id}
# argument would arrive as text. The popup starts in the pane's directory (the
# binding's -d), which is where a ticket in the branch name is read from.
#
# zsh rather than bash for vared, which edits a prefilled line; the bash that
# ships with macOS is too old for `read -i`.

setopt extendedglob

optional=${1:-}
window=$(tmux display -p '#{window_id}')
current=$(tmux show -wqv -t "$window" @tab_name)
script_dir=${0:A:h}

dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'
# Linear or Jira, as the tracker setting says, for what this says.
tracker=${${(C)$(tmux show -gqv @setting_tracker 2>/dev/null)}:-Linear}

# Esc cancels a line being typed; KEYTIMEOUT=1 keeps it from waiting for an
# escape sequence.
KEYTIMEOUT=1
bindkey -e
bindkey '\e' send-break

trim() { print -r -- ${${1##[[:space:]]#}%%[[:space:]]#}; }

save() {
  if [[ -z $1 ]]; then
    tmux set -wu -t "$window" @tab_name
  else
    tmux set -w -t "$window" @tab_name "$1"
  fi
  # Rename the tab now rather than on the next pane focus.
  "$script_dir/window-names.sh" "$(tmux display -p -t "$window" '#{session_name}')"
  exit 0
}

# The ticket the pane's branch names, if any, to fill in for t and s. Same
# rule as linear-ticket.sh and tab-label.sh.
branch_ticket=$(git branch --show-current 2>/dev/null |
  grep -ioE '[a-z][a-z0-9]*-[0-9]+' | head -1 | tr 'a-z' 'A-Z')

# The two helpers below set variables rather than print their answer, because
# $( ) would capture their prompts and messages along with it, and vared needs
# the terminal.

# Asks for a ticket, filled in from the branch, and sets $ticket to it as
# ABC-123. Accepts a Linear or Jira URL too. Fails on Esc or on anything that
# is not a ticket.
ask_ticket() {
  local input=$branch_ticket
  vared -p '  Ticket: ' input || return 1
  input=$(trim "$input")
  [[ $input =~ '/(issue|browse)/([A-Za-z][A-Za-z0-9]*-[0-9]+)' ]] && input=$match[2]
  if [[ ! $input =~ '^[A-Za-z][A-Za-z0-9]*-[0-9]+$' ]]; then
    print "\n  ${red}Not a ticket ID: ${input:-(empty)}${reset}"
    sleep 1.5
    return 1
  fi
  ticket=${(U)input}
}

# Sets $title, empty when the tracker never answered. ticket-title.sh answers from
# its cache and looks a new ticket up in the background, so the first ask comes
# back empty; someone is waiting here, so it asks again for a few seconds while
# that lookup lands.
look_up_title() {
  local tries=0
  title=
  print -n "  ${dim}Reaching out to ${tracker}...${reset}"
  while (( tries++ < 16 )); do
    title=$("$script_dir/ticket-title.sh" "$ticket")
    [[ -n $title ]] && break
    sleep 0.5
  done
  print -n '\r\e[2K'
}

print
print "  ${dim}${optional:+Optional. }Enter: type a name   t: ticket and title   s: shaping a ticket   Esc: cancel${reset}"
print
read -sk 1 choice || exit 0

case $choice in
  t|T)
    # In a ticket's own worktree the tab already shows "ABC-123: <title>" when
    # it has no name of its own (tab-label.sh), kept current if the title
    # changes in the tracker. So the name is cleared rather than copied, once it
    # has confirmed the ticket. Elsewhere, ask which ticket and save the same
    # text as the name.
    if [[ -n $branch_ticket ]]; then
      ticket=$branch_ticket
    else
      ask_ticket || exit 0
    fi
    look_up_title
    if [[ -z $title ]]; then
      print "  ${red}${tracker} has no title for $ticket, so the tab was left as it was.${reset}"
      sleep 2
      exit 0
    fi
    if [[ $ticket == $branch_ticket ]]; then
      save ""
    else
      save "$ticket: $title"
    fi
    ;;
  s|S)
    # The branch's ticket when it names one, as t does; otherwise ask.
    if [[ -n $branch_ticket ]]; then
      ticket=$branch_ticket
    else
      ask_ticket || exit 0
    fi
    save "Shaping: $ticket"
    ;;
  $'\e')
    exit 0
    ;;
  *)
    # Enter, or any other key, opens the text field with the current name. A
    # key is not taken as the first letter of a new name, since t and s would
    # then be the only letters a name could not start with.
    name=$current
    vared -p '  Tab name: ' name || exit 0
    save "$(trim "$name")"
    ;;
esac
