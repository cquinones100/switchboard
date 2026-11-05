#!/usr/bin/env bash

# Keeps tmux-resurrect's saves private and few. Run by resurrect after every
# save (@resurrect-hook-post-save-all in tmux.conf) and by every `setup.sh tmux`.
#
# With @resurrect-capture-pane-contents on, pane_contents.tar.gz holds whatever
# the panes last showed, secrets included, and resurrect writes everything
# readable by any user on the machine. This makes the directory and its files
# owner-only, and keeps only the newest KEEP layout snapshots: resurrect
# restores the one `last` points at, and continuum saves every few minutes, so
# hundreds pile up otherwise.

set -u

KEEP=50

# Resurrect's own folder, found as it finds it (its @resurrect-dir, or the
# older ~/.tmux/resurrect), so the saves tidied are the ones it makes.
dir=$("${0%/*}/resurrect-dir.sh")
[ -d "$dir" ] || exit 0

chmod 700 "$dir"
# "$dir/", so a folder reached through a link is searched too.
find "$dir/" -maxdepth 1 -type f ! -perm 600 -exec chmod 600 {} + 2>/dev/null

# Newest first by name, which is a timestamp. The one `last` points at is
# always kept, even if the clock went backwards.
last=$(readlink "$dir/last" 2>/dev/null)
ls -1 "$dir" | grep -E '^tmux_resurrect_[0-9]{8}T[0-9]{6}\.txt$' | sort -r |
  tail -n +$((KEEP + 1)) | grep -vxF "${last:-}" |
  while IFS= read -r old; do rm -f "$dir/$old"; done

exit 0
