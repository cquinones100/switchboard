#!/usr/bin/env bash
# Opens a URL in the browser, in the Chrome profile its host is given, for
# every link Switchboard opens (the link view, mission control's menus, M-r,
# M-l, the ticket and merge queue commands, and gh's --web through
# GH_BROWSER, which tmux.conf sets): the profile for a host is in
# ~/.config/switchboard/browser.json, {"hosts": {"linear.app": "Profile 2",
# "slack.com": "Profile 2"}, "default": "Profile 1"}, a host matching itself
# and its subdomains, which is yours and never in this repository. A host it
# doesn't name opens in its "default" profile, and with no default, with
# `open`, in the default browser and profile. With the chrome_profiles
# setting off, every link opens with `open`.
#
# Usage: open-url.sh <url>

set -u
url=${1:-}
[[ $url =~ ^https?:// ]] || exit 1
# Outside tmux (gh or Neovim in a terminal of its own) the setting is read
# from your settings file instead.
setting=$(tmux show -gqv @setting_chrome_profiles 2>/dev/null)
[ -n "$setting" ] || setting=$(jq -r 'if .chrome_profiles == false then "off" else empty end' "${SWITCHBOARD_SETTINGS:-$HOME/.config/switchboard/settings.json}" 2>/dev/null)
if [ "$setting" != off ]; then
  host=$(printf '%s' "$url" | sed -E 's#^https?://([^/:?\#]+).*#\1#' | tr 'A-Z' 'a-z')
  # jq, not node: this also runs from popups, whose PATH has no nvm.
  profile=$(jq -r --arg h "$host" '((.hosts // {}) | to_entries | map(select(.key as $k | $h == $k or ($h | endswith("." + $k)))) | sort_by(-(.key | length)) | .[0].value) // .default // empty' "$HOME/.config/switchboard/browser.json" 2>/dev/null)
  if [ -n "$profile" ]; then
    CHROME_PROFILE_DIR=$profile exec "${BASH_SOURCE[0]%/*}/chrome-profile.sh" "$url"
  fi
fi
exec open "$url"
