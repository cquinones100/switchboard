#!/usr/bin/env bash
# Puts tmux-continuum's autosave back in status-right when it is missing.
#
# Continuum saves from a #() job in status-right, which tmux.conf and theme.sh
# empty and then have continuum.tmux put back. continuum.tmux refuses while any
# other tmux server runs (a test's throwaway server, a switchboard-try one), so
# a reload during a test used to stop the saves until the next reload. This
# puts the job back itself, as continuum.tmux would, without that check.
#
# Usage: continuum-save.sh [--check | --start]
#   exits 0 with the job in place, putting it back first when it was missing
#   --check  changes nothing, and exits 1 when it would put the job back
#   --start  puts it back in a server that has never saved too, the first save
#            a save interval from now, as continuum's own first load does
# Each exits 2, changing nothing, when it won't: continuum isn't installed,
# this is a try's server, or (but with --start) this server has never saved,
# which means continuum held its saves back as the server started beside
# another one, and its restore with them, so a save would write over the tabs
# you saved with a server that has none of them.

set -u

check=false start=false
[ "${1:-}" = --check ] && check=true
[ "${1:-}" = --start ] && start=true

plugin=~/.config/tmux/plugins/tmux-continuum
[ -x "$plugin/scripts/continuum_save.sh" ] || exit 2
[ -n "${SWITCHBOARD_TRY_OF:-}" ] && exit 2

IFS='|' read -r saved right < <(tmux display -p '#{@continuum-save-last-timestamp}|#{status-right}' 2>/dev/null)
[[ ${right:-} == *continuum_save.sh* ]] && exit 0
[ -n "${saved:-}" ] || $start || exit 2
$check && exit 1
[ -n "${saved:-}" ] || tmux set -g @continuum-save-last-timestamp "$(date +%s)"

# The path continuum.tmux itself writes, so a later run of it finds the job
# there and doesn't add a second one.
job="#($(cd "$plugin" && pwd)/scripts/continuum_save.sh)"
tmux set -g status-right "$job${right:-}"
