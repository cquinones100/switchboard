#!/usr/bin/env bash
# Puts tmux-continuum's autosave back in status-right when it is missing.
#
# Continuum saves from a #() job in status-right, which tmux.conf and theme.sh
# empty and then have continuum.tmux put back. continuum.tmux refuses while any
# other tmux server runs (a test's throwaway server, a switchboard-try one), so
# a reload during a test used to stop the saves until the next reload. This
# puts the job back itself, as continuum.tmux would, without that check.
#
# Usage: continuum-save.sh [--check | --start | --started]
#   exits 0 with the job in place, putting it back first when it was missing
#   --check    changes nothing, and exits 1 when it would put the job back
#   --start    puts it back in a server that has never saved too, the first
#              save a save interval from now, as continuum's own first load does
#   --started  tmux.conf's, as continuum loads: as with no flag, but on a tmux
#              server of Switchboard's own (the tmux_server setting) as it
#              starts, it restores the last save (with the auto_restore setting
#              on) and starts the saves, which continuum holds back while any
#              other tmux server runs, plain ones too; settings.sh turns
#              continuum's own restore off there, so it is done once
# Each exits 2, changing nothing, when it won't: continuum isn't installed,
# this is a try's server, or (but with --start) this server has never saved,
# which means continuum held its saves back as the server started beside
# another one, and its restore with them, so a save would write over the tabs
# you saved with a server that has none of them.

set -u

check=false start=false started=false
[ "${1:-}" = --check ] && check=true
[ "${1:-}" = --start ] && start=true
[ "${1:-}" = --started ] && started=true

plugin=~/.config/tmux/plugins/tmux-continuum
[ -x "$plugin/scripts/continuum_save.sh" ] || exit 2
[ -n "${SWITCHBOARD_TRY_OF:-}" ] && exit 2

IFS='|' read -r saved server socket first right < <(tmux display -p '#{@continuum-save-last-timestamp}|#{@setting_tmux_server}|#{socket_path}|#{?@switchboard_started,,1}|#{status-right}' 2>/dev/null)
# Switchboard's own server, the first time tmux.conf loads in it.
if $started && [ -n "${first:-}" ] && [ -n "${server:-}" ] && [ "$server" != default ] && [ "${socket##*/}" = "$server" ]; then
  tmux set -g @switchboard_started 1
  restore=$(tmux show -gqv @resurrect-restore-script-path)
  if [ "$(tmux show -gqv @setting_auto_restore)" != off ] && [ ! -f ~/tmux_no_auto_restore ] && [ -x "$restore" ]; then
    # After tmux.conf has loaded, as continuum waits for it, and in the
    # background, so tmux.conf goes on loading.
    (sleep 1; "$restore") </dev/null >/dev/null 2>&1 &
  fi
  start=true
fi
[[ ${right:-} == *continuum_save.sh* ]] && exit 0
[ -n "${saved:-}" ] || $start || exit 2
$check && exit 1
[ -n "${saved:-}" ] || tmux set -g @continuum-save-last-timestamp "$(date +%s)"

# The path continuum.tmux itself writes, so a later run of it finds the job
# there and doesn't add a second one.
job="#($(cd "$plugin" && pwd)/scripts/continuum_save.sh)"
tmux set -g status-right "$job${right:-}"
