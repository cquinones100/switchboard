#!/usr/bin/env bash
# Tips: a small box in the top-right corner that teaches a key the moment it
# would help, each tip once. Any key closes it, and that key goes on to tmux
# as if the box hadn't been there, so nothing typed is lost; left alone, it
# closes after a few seconds.
#
# Usage:
#   tip.sh show <tip> [<client>]   show a tip, unless it has been shown or the
#                                  tips setting is off; on <client>, or the
#                                  client used last
#   tip.sh key <keys> [<client>]   the palette's "next time, press <keys>",
#                                  once for each key
#   tip.sh seen <tip>              mark it shown without showing it (doing what
#                                  a tip teaches, such as pressing M-a, does)
#   tip.sh load                    mark the tips shown before, at tmux start
#   tip.sh reset                   show every tip again
#   tip.sh box <tip> <client>      the popup itself
#
# What was shown is kept in ~/.local/state/switchboard/tips, one name a
# line, and as @tip_seen_<tip> options, so a hook can skip a tip with a
# format and start nothing once it has been shown.

set -u

SEEN=~/.local/state/switchboard/tips
SHOW_SECONDS=8
FOOTER="any key closes · tips are in Settings"

text_of() {
  case $1 in
    waiting) printf '%s\n' "A tab is waiting on you." "" "M-a  jump to it, then to each tab" "     that needs you after it" ;;
    jump) printf '%s\n' "Lots of tabs?" "" "M-j  find one by typing its name" "M-a  go to whichever needs you" ;;
    in-place) printf '%s\n' "This took your pane's place." "" "Quit it, or press the same key" "again, and your pane comes back." ;;
    done) printf '%s\n' "A tab finished." "" "Its mark clears once you look at it;" "M-a  visits each finished tab" ;;
    merged) printf '%s\n' "A tab's PR has merged." "" "/queue-and-clean  cleans up after a merge" "Enter in mission control's Work map" "   can remove a merged worktree" ;;
    pinned) printf '%s\n' "Pinned." "" "It stays at the top of Needs you" "until you press M-f in it again." ;;
    worktree) printf '%s\n' "Claude in a worktree." "" "M-v  the PR's description" "M-d  its diff, as GitHub shows it" "M-m  whether it can merge" ;;
    sidebar) printf '%s\n' "The sidebar." "" "Enter  go to a tab     a  answer it" "Space  peek at it      ?  every key" ;;
    loop) printf '%s\n' "A tab is looping." "" "It runs again on its own. Mission" "control (M-Space) shows when, under Loops." ;;
    updates) printf '%s\n' "Updates." "" "A git pull links what it adds too," "and the update check is in Settings." ;;
    github_off) printf '%s\n' "GitHub is off in Settings." "" "Its keys do nothing until it is on" "again: M-p, then Settings." ;;
    tracker_off) printf '%s\n' "The ticket tracker is off in Settings." "" "Its keys do nothing until it is on" "again: M-p, then Settings." ;;
    option-alt) printf '%s\n' "Option isn't acting as Alt here," "so the M- keys type letters instead." "" "~/switchboard/setup.sh terminal  fixes it" "(then restart the terminal)" ;;
    key-*) printf '%s\n' "Next time, press ${TIP_KEY:-its key}" "" "for what you just picked from M-p." ;;
    *) return 1 ;;
  esac
}

mark() {
  mkdir -p "${SEEN%/*}"
  grep -qx "$1" "$SEEN" 2>/dev/null || echo "$1" >>"$SEEN"
  tmux set -g "@tip_seen_$1" 1
}
unmark() {
  [ -f "$SEEN" ] && grep -vx "$1" "$SEEN" >"$SEEN.$$" && mv "$SEEN.$$" "$SEEN"
  [ -f "$SEEN.$$" ] && rm -f "$SEEN.$$"
  tmux set -gu "@tip_seen_$1"
}

# The key read from the box, in tmux's own names, for send-keys -K: Escape
# and a character is Alt with it, an escape sequence an arrow or such.
key_name() {
  local bytes=$1 first=${1:0:1} rest=${1:1}
  case $bytes in
    $'\x1b[A') echo Up ;; $'\x1b[B') echo Down ;; $'\x1b[C') echo Right ;; $'\x1b[D') echo Left ;;
    $'\x1b[H') echo Home ;; $'\x1b[F') echo End ;; $'\x1b[Z') echo BTab ;;
    $'\x1b') echo Escape ;;
    $'\r' | $'\n') echo Enter ;;
    $'\t') echo Tab ;;
    $'\x7f') echo BSpace ;;
    ' ') echo Space ;;
    *)
      if [ "$first" = $'\x1b' ] && [ ${#rest} -eq 1 ]; then
        echo "M-$(key_name "$rest")"
      elif [ ${#bytes} -eq 1 ] && [[ $bytes < ' ' ]]; then
        printf 'C-%b\n' "\\x$(printf '%x' $(( $(printf '%d' "'$bytes") + 96 )))"
      elif [ ${#bytes} -eq 1 ]; then
        printf '%s\n' "$bytes"
      fi
      ;;
  esac
}

case "${1:-}" in
  show)
    tip=${2:-}
    text_of "$tip" >/dev/null || exit 0
    IFS='|' read -r tips seen < <(tmux display -p "#{@setting_tips}|#{@tip_seen_$tip}" 2>/dev/null)
    [ "${tips:-}" = off ] || [ "${seen:-}" = 1 ] && exit 0
    client=${3:-$(tmux list-clients -F '#{client_activity} #{client_name}' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)}
    [ -n "$client" ] || exit 0
    mark "$tip"
    # Wide enough for its longest line and the footer, and tall enough for
    # its lines, the footer and the border.
    lines=$(text_of "$tip" | wc -l)
    width=$(( $( { text_of "$tip"; echo "$FOOTER"; } | awk '{ if (length > n) n = length } END { print n }') + 4 ))
    # A client shows one popup at a time: a tip that couldn't open, behind
    # another popup, is left to show another time.
    tmux display-popup -c "$client" -e "TIP_KEY=${TIP_KEY:-}" -E -x R -y 0 -w "$width" -h $(( lines + 4 )) -b rounded -T ' tip ' "$0 box $(printf '%q %q' "$tip" "$client")" ||
      unmark "$tip"
    ;;
  # A key picked from the palette that has a hotkey of its own: the key as the
  # palette shows it (Shift spelled out, M-S-W), once for each key.
  key)
    choice=${2:-}
    [ -n "$choice" ] && [[ $choice != @palette* ]] || exit 0
    tip="key-$(printf '%s' "$choice" | sed 's/[A-Z]/s&/g' | tr 'A-Z' 'a-z' | tr -c 'a-z0-9' '-')"
    TIP_KEY=$(printf '%s' "$choice" | sed -E 's/(^|[ -])([A-Z])( |$)/\1S-\2\3/g')
    export TIP_KEY
    exec "$0" show "$tip" ${3:+"$3"}
    ;;
  seen) [ -n "${2:-}" ] && mark "$2" ;;
  load)
    [ -f "$SEEN" ] || exit 0
    while read -r tip; do
      [[ $tip =~ ^[a-z0-9_-]+$ ]] && tmux set -g "@tip_seen_$tip" 1
    done <"$SEEN"
    ;;
  reset)
    rm -f "$SEEN"
    tmux show-options -g 2>/dev/null | awk '/^@tip_seen_/ { print $1 }' | while read -r option; do tmux set -gu "$option"; done
    ;;
  box)
    tip=${2:-} client=${3:-}
    printf '\033[?25l'
    text_of "$tip" | sed 's/^/ /'
    printf '\n \033[38;5;244m%s\033[0m' "$FOOTER"
    IFS= read -rsn 1 -t "$SHOW_SECONDS" first || exit 0
    # What else the key sent, as an arrow or Alt sends several bytes at once.
    keys=$first
    while IFS= read -rsn 1 -t 0.02 more; do keys+=$more; done
    [ -z "$first" ] && keys=$'\r'
    name=$(key_name "$keys")
    # Sent by tmux a moment after the box has closed, so the key reaches what
    # was under it, a binding included. ## keeps a # from being read as a format.
    [ -n "$name" ] && [ -n "$client" ] &&
      tmux run-shell -b -d 0.1 "tmux send-keys -K -c $(printf %q "$client") $(printf %q "$name" | sed 's/#/##/g')"
    ;;
esac
exit 0
