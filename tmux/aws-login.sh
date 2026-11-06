#!/usr/bin/env bash
# Logs in to AWS IAM Identity Center (`aws sso login`), from the M-p palette's popup,
# opening the login page in a chosen Chrome profile rather than whichever
# Chrome window is in front. The popup stays open until the login finishes, so
# the verification code and any error can be read, then waits for a key.
#
# No profile is named: it logs in to the one [sso-session] in ~/.aws/config,
# which covers every profile that uses it. Logging in through a profile that
# has its own sso_start_url instead is cached apart and doesn't cover those.
# With several sessions, it says which there are rather than guess.
#
# Which Chrome profile is yours, not this public repository's: the one
# ~/.config/switchboard/browser.json gives awsapps.com, AWS's access portal,
# as it gives every site its profile ({"hosts": {"awsapps.com": "Profile 2"}}),
# else its default profile ("default"); without either, Chrome picks.

dim=$'\033[2m' red=$'\033[31m' green=$'\033[32m' reset=$'\033[0m'

done_with() {
  printf '\n  %s\n\n  %sPress any key to close.%s' "$1" "$dim" "$reset"
  read -rsn 1
  exit 0
}

chrome_profile=$(jq -r '.hosts["awsapps.com"] // .default // empty' ~/.config/switchboard/browser.json 2>/dev/null)
browser=~/.config/tmux/chrome-profile.sh
# The chrome_profiles setting off: the default browser, as every link then.
[ "$(tmux show -gqv @setting_chrome_profiles 2>/dev/null)" = off ] && browser=open
command -v aws >/dev/null 2>&1 || done_with "${red}aws isn't on the PATH.${reset} Install it with: brew install awscli"

sessions=$(sed -n 's/^\[sso-session \(.*\)\]$/\1/p' ~/.aws/config 2>/dev/null)
case $(printf '%s' "$sessions" | grep -c .) in
  0) done_with "${red}There is no [sso-session] in ~/.aws/config.${reset}" ;;
  1) ;;
  *) done_with "${red}There are several SSO sessions in ~/.aws/config:${reset} $(printf '%s' "$sessions" | paste -sd ' ' -)" ;;
esac

printf '\n  %sLogging in to %s...%s\n\n' "$dim" "$sessions" "$reset"
# BROWSER for this command alone (see chrome-profile.sh).
# The request audit (count-request.sh): the login, as AWS's SSO sees it.
{ . "${BASH_SOURCE[0]%/*}/count-request.sh" && count_request AWS HTTPS 'AWS SSO login' aws; } 2>/dev/null
if BROWSER=$browser CHROME_PROFILE_DIR=$chrome_profile aws sso login --sso-session "$sessions"; then
  done_with "${green}Logged in.${reset}"
else
  done_with "${red}The login failed.${reset}"
fi
