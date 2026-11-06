#!/usr/bin/env bash

# Toggles or sets the tmux color theme. Persists the chosen mode to
# ~/.config/tmux/.theme-mode so it survives server restarts (tmux-continuum
# does not save styling), and reapplies accent-aware styles for the current
# pane via update-colors.sh.
#
# Usage: theme.sh [dark|light|toggle]
#   default: toggle

script_dir="$(dirname "$0")"
state_file="$script_dir/.theme-mode"

# Ghostty themes — override via env if you want different ones.
# GitHub Light Default has noticeably darker bright_black than Tomorrow,
# so TUI dim/placeholder text stays readable on white.
GHOSTTY_THEME_DARK="${GHOSTTY_THEME_DARK:-Tomorrow Night}"
GHOSTTY_THEME_LIGHT="${GHOSTTY_THEME_LIGHT:-GitHub Light High Contrast}"

mode="${1:-toggle}"

if [ "$mode" = "toggle" ]; then
  current=$(cat "$state_file" 2>/dev/null || echo dark)
  if [ "$current" = "dark" ]; then mode=light; else mode=dark; fi
fi

case "$mode" in
  dark)
    BG_DARK=colour235; BG_MED=colour238; BG_DIM=colour236
    BUTTON="bg=colour238,fg=colour255"
    FG=colour248; FG_DIM=colour244; FG_BRIGHT=colour255
    GHOSTTY_THEME="$GHOSTTY_THEME_DARK"
    ;;
  light)
    BG_DARK=colour253; BG_MED=colour248; BG_DIM=colour254
    BUTTON="bg=colour250,fg=colour232"
    FG=colour234; FG_DIM=colour238; FG_BRIGHT=colour232
    GHOSTTY_THEME="$GHOSTTY_THEME_LIGHT"
    ;;
  *)
    echo "Usage: $0 [dark|light|toggle]" >&2
    exit 1
    ;;
esac

echo "$mode" > "$state_file"

tmux set -g status-style "bg=$BG_DARK,fg=$FG"
tmux set -g status-right ""

# theme.sh overwrites status-right, where tmux-continuum keeps its periodic-save
# hook; re-run the plugin to put it back.
continuum_tmux=~/.config/tmux/plugins/tmux-continuum/continuum.tmux
[ -x "$continuum_tmux" ] && "$continuum_tmux"
tmux setw -g window-status-format "#[fg=$FG_DIM] #I:#{E:@agent_marker}#[bg=default,fg=$FG_DIM,nobold]#{?@agent, ,}#W "
# Buttons in the status line (the Commands button, @palette_button in tmux.conf).
tmux set -g @button_style "$BUTTON"
tmux set -g pane-border-style "fg=$BG_MED"
tmux set -g window-style "bg=$BG_DIM"
tmux set -g window-active-style "bg=terminal"
tmux set -g message-style "bg=$BG_DARK,fg=$FG"
tmux set -g message-command-style "bg=$BG_DARK,fg=$FG"

pane_path=$(tmux display -p '#{pane_current_path}')
session=$(tmux display -p '#{session_name}')
"$script_dir/update-colors.sh" "$pane_path" "$session"

# Switch Ghostty theme if it's running. Writes a small include file the main
# config picks up via `config-file = ?~/.config/ghostty/theme-mode.conf`,
# then sends SIGUSR2 to make Ghostty reload its config in place.
ghostty_conf="$HOME/.config/ghostty/theme-mode.conf"
mkdir -p "$(dirname "$ghostty_conf")"
printf 'theme = %s\n' "$GHOSTTY_THEME" > "$ghostty_conf"
pkill -USR2 -x ghostty 2>/dev/null || true
