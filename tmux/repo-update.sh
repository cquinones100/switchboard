#!/usr/bin/env bash
# Keeps ~/switchboard up to date with GitHub: fetches origin, and when its
# main has commits yours doesn't, offers them in a popup that fast-forwards
# main, runs ./setup.sh, and reloads tmux.
#
# Usage:
#   repo-update.sh check [<client>]   fetch, and offer any new commits in a
#                                      popup on <client>, or the client used
#                                      last. Run when a client attaches or a
#                                      session starts (tmux.conf), so it asks
#                                      whenever there is something new.
#   repo-update.sh periodic           the same, at most every CHECK_EVERY
#                                      seconds, asking again about a tip until
#                                      it is answered, so "Not now" isn't
#                                      asked again until more commits land.
#                                      Started by health.sh.
#   repo-update.sh now <client>       M-p's "Update Switchboard": the same as
#                                      check, whatever the update_check setting
#                                      and however recently it looked; with
#                                      nothing new, or GitHub out of reach, it
#                                      says so on <client> and runs the guided
#                                      setup all the same, for a clone pulled
#                                      by hand
#   repo-update.sh prompt [<client>]  the popup itself, on <client>.
#   repo-update.sh setup <client>     M-p's "Run Switchboard's setup": the guided
#                                      ./setup.sh in a popup, whenever you like,
#                                      to set up anything not in place yet or
#                                      answer a question you said no to before
#
# It runs in the background, never on a key press or redraw, since it waits
# on the network. Main is only fast-forwarded: with commits of your own not
# on GitHub, or another branch checked out, it says so and changes nothing.

# The request audit (count-request.sh); counting nothing when it isn't there.
. "${BASH_SOURCE[0]%/*}/count-request.sh" 2>/dev/null || { count_request() { :; }; count_git_request() { :; }; }
set -u
# The menus and the "Press any key to close" boxes (menu.sh).
. "${BASH_SOURCE[0]%/*}/menu.sh"
# Its lists look like mission control's boxes (fzf-style.sh).
export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$("${BASH_SOURCE[0]%/*}/fzf-style.sh" 2>/dev/null)"

REPO="${SWITCHBOARD_DIR:-$HOME/switchboard}"
# A clone not yet set up under its new name (setup.sh links it).
[ -d "$REPO" ] || REPO=$HOME/editor-configs
STATE=~/.local/state/switchboard
CHECK_EVERY=3600

git_() { git -C "$REPO" "$@"; }

case "${1:-}" in
  check | periodic | now)
    # The update_check setting (tmux/settings.sh): off never checks, and
    # "open" leaves out the hourly one. Asked for, it always looks.
    case "$1:$(tmux show -gqv @setting_update_check 2>/dev/null)" in
      check:off | periodic:off | periodic:open) exit 0 ;;
    esac
    # What happened, said on the client that asked; nothing for the others.
    say() { [ "$1" = now ] && "${0%/*}/say.sh" -c "${2:-}" "$3"; return 0; }
    # Asked for with nothing new to take, or GitHub out of reach, the setup
    # still runs, saying why first, so a clone pulled by hand (or changed some
    # other way) gets whatever its setup has to do.
    setup_anyway() {
      [ "$1" = now ] || return 0
      "${0%/*}/say.sh" close "${2:-}"
      tmux display-popup -c "${2:-}" -E -w 90 -h 85% -T '#[bold] Switchboard setup ' "SETUP_WHY=$(printf %q "$3") $0 run-setup"
    }
    mkdir -p "$STATE"
    now=$(date +%s)
    if [ "$1" = periodic ]; then
      read -r last 2>/dev/null < "$STATE/update-checked"
      [ $((now - ${last:-0})) -lt $CHECK_EVERY ] && exit 0
    fi
    # Recorded first, so health.sh, which starts this when the hour is up,
    # doesn't start it again on every redraw while it runs or has nothing to do.
    echo "$now" > "$STATE/update-checked"
    [ -d "$REPO/.git" ] || { say "$1" "${2:-}" "Switchboard isn't a git clone at $REPO, so it can't update."; exit 0; }
    # One check at a time; a lock left by a check that died goes after ten minutes.
    lock="$STATE/update.lock"
    if ! mkdir "$lock" 2>/dev/null; then
      [ -n "$(find "$lock" -maxdepth 0 -mmin +10 2>/dev/null)" ] || { say "$1" "${2:-}" "Switchboard is already looking for updates."; exit 0; }
    fi
    trap 'rmdir "$lock" 2>/dev/null' EXIT
    # Your CLAUDE.md's rules, when they live in a git repository (Settings'
    # Your CLAUDE.md): pulled with the same look, fast-forward only, by setup's
    # CLAUDE.md step, which also writes ~/.claude/CLAUDE.md again if it must.
    # It never asks for anything: git's own prompt is off, and so is its askpass
    # (GIT_ASKPASS=false, which fails at once; a credential helper such as the
    # Keychain still answers first), ssh may not open an askpass dialog
    # (SSH_ASKPASS_REQUIRE=never), and this runs from the tmux
    # server, with no terminal for ssh to ask on; your ssh settings
    # (GIT_SSH_COMMAND, GIT_SSH, the clone's core.sshCommand, ~/.ssh/config)
    # are left as they are. It runs beside the Switchboard look, not before
    # it, so nothing it waits on can hold that up, and only pulls a clone
    # already there (SWITCHBOARD_PULL_ONLY), cloning being setup's.
    if [ -n "$(jq -r '.repo // empty' ~/.config/switchboard/claude.json 2>/dev/null)" ] && [ -x "$REPO/setup.sh" ]; then
      GIT_TERMINAL_PROMPT=0 GIT_ASKPASS=false SSH_ASKPASS_REQUIRE=never SWITCHBOARD_PULL_ONLY=true nohup bash "$REPO/setup.sh" claude_md >/dev/null 2>&1 </dev/null &
    fi
    [ "$1" = now ] && "${0%/*}/say.sh" -c "${2:-}" "Looking for Switchboard updates…"
    count_git_request "$REPO" 'Switchboard update check'
    GIT_TERMINAL_PROMPT=0 GIT_SSH_COMMAND="ssh -o BatchMode=yes" git_ fetch --quiet origin main 2>/dev/null || { setup_anyway "$1" "${2:-}" "Couldn't reach GitHub to look for updates; running the setup with what you have."; exit 0; }
    behind=$(git_ rev-list --count main..origin/main 2>/dev/null) || exit 0
    [ "${behind:-0}" -gt 0 ] || { setup_anyway "$1" "${2:-}" "Switchboard is up to date ($(git_ log -1 --format=%h main)); running its setup for anything not in place yet."; exit 0; }
    tip=$(git_ rev-parse origin/main)
    if [ "$1" = periodic ]; then
      read -r offered 2>/dev/null < "$STATE/update-offered"
      [ "${offered:-}" = "$tip" ] && exit 0
    fi
    client="${2:-$(tmux list-clients -F '#{client_activity} #{client_name}' 2>/dev/null | sort -rn | head -1 | cut -d' ' -f2-)}"
    [ -n "$client" ] || exit 0
    # A prompt still open from before (shown while you were away, say; its pid
    # still running this script, not one reused since): left
    # as it is when it offers this same tip on this client, else closed for
    # this one, so coming back you see what is new now. Only an answer
    # (update-offered, written by the prompt) keeps the hourly look from
    # asking again about a tip.
    if read -r open_pid open_tip open_client 2>/dev/null < "$STATE/update-prompt" && ps -o command= -p "$open_pid" 2>/dev/null | grep -q 'repo-update.sh prompt'; then
      [ "$open_tip" = "$tip" ] && [ "$open_client" = "$client" ] && exit 0
      tmux display-popup -C -c "$open_client" 2>/dev/null
      for _ in 1 2 3 4 5 6 7 8 9 10; do kill -0 "$open_pid" 2>/dev/null || break; sleep 0.1; done
    fi
    # The look is done: its lock goes before the popup, which waits on you,
    # so a look from another client meanwhile can replace it.
    rmdir "$lock" 2>/dev/null
    trap - EXIT
    "${0%/*}/say.sh" close "$client"
    tmux display-popup -c "$client" -E -w 90 -h 85% -T '#[bold] Switchboard ' "$0 prompt $(printf %q "$client")"
    # How updates also arrive, once (tip.sh), after a prompt answered: one
    # closed for a newer prompt leaves the client to that one.
    read -r answered_tip 2>/dev/null < "$STATE/update-offered"
    [ "${answered_tip:-}" = "$tip" ] && [ -x ~/.config/tmux/tip.sh ] && ~/.config/tmux/tip.sh show updates "$client"
    ;;

  prompt)
    # The new commits, newest first, each its subject wrapped to the popup
    # with its hash and age under it, as many as fit the popup (eight at most;
    # "See all" pages through every one), then a menu
    # moved through with j and k (or the arrows) and taken with Enter, "Update
    # now" first, so Enter alone still updates. Esc or q is "Not now"; a
    # closed input answers nothing.
    tip=$(git_ rev-parse origin/main)
    mkdir -p "$STATE"
    echo "$$ $tip ${2:-}" > "$STATE/update-prompt"
    trap 'read -r pid _ < "$STATE/update-prompt" 2>/dev/null; [ "${pid:-}" = $$ ] && rm -f "$STATE/update-prompt"' EXIT
    answered() { echo "$tip" > "$STATE/update-offered"; }
    behind=$(git_ rev-list --count main..origin/main)
    ahead=$(git_ rev-list --count origin/main..main)
    cols=$(stty size 2>/dev/null </dev/tty | cut -d' ' -f2)
    [ "${cols:-0}" -gt 20 ] 2>/dev/null || cols=88
    height=$(stty size 2>/dev/null </dev/tty | cut -d' ' -f1)
    [ "${height:-0}" -gt 5 ] 2>/dev/null || height=24
    # Room for the commits: the heading, the "more" line, the menu and its
    # keys take eleven rows; each commit its wrapped subject and two more.
    # A subject too long for the room alone is cut to it (CAP lines), the
    # whole of it behind "See all".
    room=$((height - 11)) SHOWN=0 used=0 CAP=$((height - 13)) cut_short=
    [ $CAP -ge 1 ] || CAP=1
    while IFS= read -r subject; do
      need=$(($(printf '%s\n' "$subject" | fold -s -w $((cols - 8)) | wc -l) + 2))
      [ $SHOWN -gt 0 ] && [ $((used + need)) -gt $room ] && break
      [ $((need - 2)) -gt $CAP ] && cut_short=1 need=$((CAP + 2))
      used=$((used + need)) SHOWN=$((SHOWN + 1))
      [ $SHOWN -ge 8 ] && break
    done < <(git_ log --format=%s main..origin/main)
    # One commit: a dot, its subject wrapped, then its hash and age, dimmed.
    commits() {
      git_ log --format='%h%x09%cr%x09%s' "$@" main..origin/main | while IFS=$'\t' read -r hash age subject; do
        printf '%s\n' "$subject" | fold -s -w $((cols - 8)) | head -n "${CAP:-1000000}" | sed '1s/^/  ● /; 2,$s/^/    /'
        printf '    \033[38;5;244m%s · %s\033[0m\n\n' "$hash" "$age"
      done
    }
    heading() { printf '\n  \033[1m%s new commit%s on GitHub\033[0m, newest first:\n\n' "$behind" "$([ "$behind" = 1 ] || echo s)"; }
    done_() {
      printf '\n  \033[38;5;244mPress any key to close.\033[0m'
      menu_any_key
      exit "${1:-0}"
    }
    # A notice that it can't update here comes before every commit, whole, in
    # less when there is a terminal; seen (less closed, or a key, not a closed
    # input), it is an answer: asked again only when more commits land.
    noticed() {
      if [ -t 1 ]; then
        { printf '\n  %s\n' "$1"; heading; CAP= commits; } | less -R
        answered
      else
        printf '\n  %s\n' "$1"
        heading
        CAP= commits
        printf '\n  \033[38;5;244mPress any key to close.\033[0m'
        menu_any_key && answered
      fi
      exit 0
    }
    [ "$ahead" -gt 0 ] && noticed "You have $ahead commit$([ "$ahead" = 1 ] || echo s) of your own not on GitHub, so main can't simply move forward. Update it by hand."
    [ "$(git_ branch --show-current)" = main ] || noticed "$REPO has $(git_ branch --show-current) checked out, not main. Update it by hand."
    options=("Update now" "Not now")
    { [ "$behind" -gt "$SHOWN" ] || [ -n "$cut_short" ]; } && options+=("See all $behind commit$([ "$behind" = 1 ] || echo s)")
    cursor=0
    # A click takes an option too (menu.sh, the menu_mouse setting).
    menu_mouse_on
    while :; do
      printf '\033[H\033[2J'
      heading
      commits -n "$SHOWN"
      [ "$behind" -gt "$SHOWN" ] && printf '  \033[38;5;244m…and %s more\033[0m\n\n' $((behind - SHOWN))
      for i in "${!options[@]}"; do
        menu_option "$([ "$i" = "$cursor" ] && echo 1)" "${options[$i]}"
      done
      menu_keys "jk or tab move  ↵ or click choose  esc not now"
      # Where the first option is, for a click's row (only on a terminal):
      # asked once the keys are drawn, which can scroll the popup.
      start=
      [ -t 0 ] && [ -n "${MENU_MOUSE:-}" ] && start=$(( $(menu_row) - ${#options[@]} - 1 ))
      # The cursor past the options is on the close button (menu.sh).
      MENU_CLOSE_FOCUS=$([ "$cursor" = ${#options[@]} ] && echo 1)
      # A closed input answers nothing: the prompt goes, and asks again later.
      menu_read || { menu_mouse_off; exit 0; }
      [ -n "$MENU_CLOSE_FOCUS" ] && case $MENU_KEY in enter | space) MENU_KEY=quit ;; esac
      pick=
      case $MENU_KEY in
        j) [ $cursor -lt $((${#options[@]} - 1)) ] && cursor=$((cursor + 1)) ;;
        k) [ $cursor -gt 0 ] && cursor=$((cursor - 1)) ;;
        tab) cursor=$(( (cursor + 1) % $(menu_stops ${#options[@]}) )) ;;
        btab) cursor=$(( (cursor + $(menu_stops ${#options[@]}) - 1) % $(menu_stops ${#options[@]}) )) ;;
        quit) menu_mouse_off; answered; exit 0 ;;
        enter | space) pick=$cursor ;;
        click\ *)
          set -- $MENU_KEY
          i=$(( $2 - ${start:-0} ))
          [ -n "$start" ] && [ "$i" -ge 0 ] && [ "$i" -lt ${#options[@]} ] && cursor=$i && pick=$i
          ;;
      esac
      if [ -n "$pick" ]; then
        case ${options[$pick]} in
          "Update now") break ;;
          "Not now") menu_mouse_off; answered; exit 0 ;;
          *) menu_mouse_off; { heading; CAP= commits; } | less -R; menu_mouse_on ;;
        esac
      fi
    done
    menu_mouse_off
    answered
    # Answered, it is no longer a prompt a newer look may close: the update
    # and its setup run on undisturbed.
    rm -f "$STATE/update-prompt"
    printf '\033[H\033[2J\n  \033[2mUpdating…\033[0m\n'
    old=$(git_ rev-parse HEAD)
    if ! out=$(git_ merge --ff-only --quiet origin/main 2>&1); then
      printf '  It could not update:\n\n%s\n' "$(sed 's/^/  /' <<<"$out")"
      done_ 1
    fi
    printf '  Updated to %s.\n' "$(git_ log -1 --format=%h)"
    # The guided setup, which links what is new, installs the pinned tmux
    # plugins and anything else the update needs, and asks about the rest. It
    # changes only what isn't in place. Not piped, so its questions show.
    if [ -x "$REPO/setup.sh" ]; then
      printf '\n  \033[2mRunning the setup, to link and install what the update brought…\033[0m\n\n'
      "$REPO/setup.sh"
      printf '\n'
    fi
    if [ -n "${TMUX:-}" ]; then
      # The sidebar daemon keeps running the code it started with; reloading
      # tmux.conf starts it again once it has stopped.
      if [ -n "$(git_ diff --name-only "$old" HEAD -- lib/sidebar bin/tmux-sidebar)" ]; then
        kill "$(cut -d' ' -f1 ~/.local/state/tmux-sidebar/daemon.pid 2>/dev/null)" 2>/dev/null && sleep 1 && printf '  Restarted the sidebar.\n'
      fi
      tmux source-file ~/.config/tmux/tmux.conf && printf '  Reloaded tmux.\n'
    fi
    done_
    ;;

  setup)
    tmux display-popup -c "${2:-}" -E -w 90 -h 85% -T '#[bold] Switchboard setup ' "$0 run-setup"
    ;;

  # M-p's "Check Switchboard's setup": what the guided setup would do, changing nothing.
  setup-check)
    tmux display-popup -c "${2:-}" -E -w 90 -h 85% -T '#[bold] Switchboard setup, checked ' "$0 run-setup-check"
    ;;

  run-setup-check)
    printf '\n'
    if [ -x "$REPO/setup.sh" ]; then
      "$REPO/setup.sh" --check
    else
      printf '  There is no setup.sh in %s.\n' "$REPO"
    fi
    printf '\n  \033[38;5;244mPress any key to close.\033[0m'
    menu_any_key
    ;;

  # M-p's "Run a part of Switchboard's setup": the parts setup.sh lists, Tab
  # choosing several; Enter runs them, Ctrl-O only checks them.
  setup-part)
    tmux display-popup -c "${2:-}" -E -w 90 -h 85% -T '#[bold] A part of Switchboard setup ' "$0 run-setup-part"
    ;;

  run-setup-part)
    [ -x "$REPO/setup.sh" ] || { printf '\n  There is no setup.sh in %s.\n\n  Press any key to close.' "$REPO"; menu_any_key; exit 0; }
    picked=$("$REPO/setup.sh" --parts | fzf --multi --delimiter=$'\t' --with-nth=1,2 --layout=reverse --height=100% \
      --prompt='part> ' --header='Enter runs it (Tab picks several), Ctrl-O only checks what it would change' --expect=ctrl-o)
    key=$(head -1 <<<"$picked")
    chosen=()
    while IFS=$'\t' read -r part _; do [ -n "$part" ] && chosen+=("$part"); done < <(tail -n +2 <<<"$picked")
    [ ${#chosen[@]} -gt 0 ] || exit 0
    printf '\n'
    if [ "$key" = ctrl-o ]; then
      "$REPO/setup.sh" --check "${chosen[@]}"
    else
      "$REPO/setup.sh" "${chosen[@]}"
      [ -n "${TMUX:-}" ] && tmux source-file ~/.config/tmux/tmux.conf && printf '\n  Reloaded tmux.\n'
    fi
    printf '\n  \033[38;5;244mPress any key to close.\033[0m'
    menu_any_key
    ;;

  run-setup)
    # The guided setup, as an update runs it: it changes only what isn't in
    # place and asks before anything else. Then tmux takes in what it linked.
    printf '\n'
    [ -n "${SETUP_WHY:-}" ] && printf '  %s\n\n' "$SETUP_WHY"
    if [ -x "$REPO/setup.sh" ]; then
      "$REPO/setup.sh"
    else
      printf '  There is no setup.sh in %s.\n' "$REPO"
    fi
    [ -n "${TMUX:-}" ] && tmux source-file ~/.config/tmux/tmux.conf && printf '\n  Reloaded tmux.\n'
    printf '\n  \033[38;5;244mPress any key to close.\033[0m'
    menu_any_key
    ;;

  *)
    echo "Usage: $0 check [<client>] | periodic | now <client> | prompt | setup <client> | setup-check <client> | setup-part <client>" >&2
    exit 1
    ;;
esac
