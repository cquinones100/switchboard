#!/usr/bin/env bash
# Applies the workspace's settings to tmux. Every option is listed in
# tmux/settings.json with its default; what you changed is in
# ~/.config/switchboard/settings.json (mission control's Settings view
# writes it, and you can edit it by hand). Each value becomes a global tmux
# option, @setting_<key> (on/off, or the choice's value), which the scripts
# and hooks read without starting anything, and the few that are tmux's own
# settings are set here too.
#
# Usage:
#   settings.sh apply [--load]   set the options; tmux.conf runs it with
#                                --load before the plugins (continuum reads
#                                its restore setting as it loads), and the
#                                sidebar daemon runs it whenever the file
#                                changes. --load leaves the theme to `theme`.
#   settings.sh theme            apply the theme setting (tmux.conf, after
#                                the plugins, as theme.sh always ran)
#   settings.sh set <key> <value>   change one setting, then apply
#   settings.sh toggle-theme     between dark and light, as the palette's
#                                theme toggle does, keeping it as the setting

set -u
umask 077

REPO="${SWITCHBOARD_DIR:-$HOME/switchboard}"
# A clone not yet set up under its new name (setup.sh links it).
[ -d "$REPO" ] || REPO=$HOME/editor-configs
SCHEMA="$REPO/tmux/settings.json"
FILE="${SWITCHBOARD_SETTINGS:-$HOME/.config/switchboard/settings.json}"
script_dir="$(dirname "$0")"

# What you set, or {} when the file is missing or isn't a JSON object. Linear
# turned off, from before the tracker setting, is the tracker turned off, as
# lib/settings.js reads it.
mine() {
  jq -c 'if type == "object" then . else {} end
    | if .tracker == null and .linear == false then .tracker = "off" else . end | del(.linear)' "$FILE" 2>/dev/null || echo '{}'
}

# key<TAB>value for every option, the default standing in for a value that
# isn't one of its own; on/off for the on/off ones.
values() {
  jq -r --argjson mine "$(mine)" '
    .[] | . as $o | $mine[$o.key] as $v
    | (if $o.choices then (if any($o.choices[]; .value == $v) then $v else $o.default end)
       else (if ($v | type) == "boolean" then $v else $o.default end) end) as $val
    | "\($o.key)\t\(if $val == true then "on" elif $val == false then "off" else $val end)"
  ' "$SCHEMA"
}

value_of() { values | awk -F'\t' -v k="$1" '$1 == k { print $2 }'; }

# dark or light, from the theme setting; system asks macOS.
theme_mode() {
  local theme
  theme=$(value_of theme)
  if [ "$theme" = system ]; then
    [ "$(defaults read -g AppleInterfaceStyle 2>/dev/null)" = Dark ] && echo dark || echo light
  else
    echo "${theme:-dark}"
  fi
}

apply_theme() {
  "$script_dir/theme.sh" "$(theme_mode)"
}

apply() {
  local load=${1:-} key value prev changed=() args=()
  # What was applied last, to act only on what changed.
  local before
  before=$(tmux show-options -g 2>/dev/null | grep '^@setting_')
  while IFS=$'\t' read -r key value; do
    prev=$(awk -v k="@setting_$key" '$1 == k { print $2 }' <<<"$before")
    prev=${prev//\"/}
    [ "$prev" != "$value" ] && changed+=("$key")
    args+=(set -g "@setting_$key" "$value" \;)
    case $key in
      second_status) args+=(set -g status "$([ "$value" = on ] && echo 2 || echo on)" \;) ;;
      save_contents) args+=(set -g @resurrect-capture-pane-contents "$value" \;) ;;
      auto_restore) args+=(set -g @continuum-restore "$value" \;) ;;
      # Only when the setting itself changes, so M-s's hide-for-now holds.
      sidebar)
        if [ "$prev" != "$value" ]; then
          if [ "$value" = on ]; then args+=(set -gu @sidebar_on \;); else args+=(set -g @sidebar_on off \;); fi
        fi
        ;;
    esac
  done < <(values)
  (( ${#args[@]} )) || return 1
  tmux "${args[@]}"
  disable_services
  [ -n "$load" ] && return 0
  # The rest only when tmux is already running with the old values.
  local list=" ${changed[*]} "
  [[ $list == *" theme "* ]] && apply_theme
  # A service turned back on gets its keys back from tmux.conf, which runs
  # this again (with --load) for whatever is still off.
  if [[ ( $list == *" github "* && $(value_of github) == on ) || ( $list == *" tracker "* && $(value_of tracker) != off ) ]]; then
    tmux source-file ~/.config/tmux/tmux.conf
  fi
  if [[ $list == *" accent_colors "* || $list == *" tab_lookups "* || $list == *" github "* || $list == *" tracker "* ]]; then
    # Tab colors and names are cached per directory (path-facts.sh), and
    # ticket titles per ID, whichever tracker they came from (ticket-title.sh).
    rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/tmux-path-facts"
    [[ $list == *" tracker "* ]] && rm -rf "${XDG_CACHE_HOME:-$HOME/.cache}/tmux-ticket-title"
    tmux list-sessions -F '#{session_name}' 2>/dev/null | while read -r session; do
      "$script_dir/window-names.sh" "$session"
    done
  fi
  [[ $list == *" sidebar "* ]] && "$script_dir/sidebar.sh" nudge
  return 0
}

# With the github or tracker setting off, every key whose description carries
# that service's tag (#github, #tracker) is bound again to do nothing, in the
# root, prefix and palette tables, so pressing it neither asks the service nor
# reaches the pane, but says why the first time (tip.sh). Bound without a
# note, it also drops out of the palette.
disable_services() {
  local service table key
  for service in github tracker; do
    [ "$(value_of "$service")" = off ] || continue
    for table in root prefix palette; do
      tmux list-keys -N -T "$table" -P '' 2>/dev/null | grep -E " #$service( |$)" | awk '{ print $1 }' |
        while read -r key; do
          tmux bind -T "$table" "$key" if -F "#{@tip_seen_${service}_off}" "" "run-shell -b '~/.config/tmux/tip.sh show ${service}_off #{q:client_name}'"
        done
    done
  done
}

# Writes one setting, leaving out a value equal to its default, through a
# temporary file renamed into place.
set_one() {
  local key=$1 value=$2 json
  json=$(jq -c --arg k "$key" --arg v "$value" '
    .[] | select(.key == $k)
    | if .choices then (if any(.choices[]; .value == $v) then {default, value: $v} else empty end)
      elif $v == "on" or $v == "true" then {default, value: true}
      elif $v == "off" or $v == "false" then {default, value: false}
      else empty end' "$SCHEMA")
  if [ -z "$json" ]; then
    echo "not a setting and value: $key $value" >&2
    return 1
  fi
  mkdir -p "$(dirname "$FILE")"
  mine | jq --arg k "$key" --argjson pick "$json" 'if $pick.value == $pick.default then del(.[$k]) else .[$k] = $pick.value end' >"$FILE.$$.tmp" &&
    mv "$FILE.$$.tmp" "$FILE"
}

case "${1:-}" in
  apply) apply "${2:-}" ;;
  theme) apply_theme ;;
  set) set_one "${2:-}" "${3:-}" && apply ;;
  toggle-theme)
    current=$(cat "$script_dir/.theme-mode" 2>/dev/null || echo dark)
    set_one theme "$([ "$current" = dark ] && echo light || echo dark)" && apply
    ;;
  *)
    echo "Usage: $0 apply [--load] | theme | set <key> <value> | toggle-theme" >&2
    exit 1
    ;;
esac
