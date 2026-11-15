#!/bin/bash
# Keeps Claude from getting around git hooks: no `git commit --no-verify` (or
# its short form -n, or an abbreviation git takes, such as --no-veri), and
# no changing core.hooksPath, which would switch a
# repository's hooks off. Switchboard relies on its pre-push hook to keep an
# unreviewed commit off main. Reading the setting is fine. Registered only in
# this repository's own .claude/settings.json, so it applies to Claude
# working here and nowhere else.
#
# Without jq, or with JSON it can't read, it refuses rather than let the
# command through unread.
deny() {
  echo "$1" >&2
  exit 2
}
command -v jq >/dev/null 2>&1 || deny "This repository's git hook guard needs jq to read the command: install it."
INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty') ||
  deny "This repository's git hook guard couldn't read the command it was given."
[[ -n "$CMD" ]] || exit 0
# Global options before the subcommand, including the ones that take a value (git -C <path> commit).
VALUE='("[^"]*"|'\''[^'\'']*'\''|\S+)'
OPTION="(-[Cc]\s+${VALUE}|--(git-dir|work-tree|namespace|config-env|exec-path)\s+${VALUE}|-\S+)"
# git, or git by its path (/usr/bin/git).
GIT="(^|;|&&|\|\||\|)\s*(\S*/)?git\s+(${OPTION}\s+)*"
# Every line is read, lines a backslash continues joined first. Quoted
# strings are dropped first, so a commit message that mentions -n or
# --no-verify is not mistaken for the flag.
JOINED=$(printf '%s\n' "$CMD" | sed -e ':a' -e '/\\$/N; s/\\\n//; ta')
while IFS= read -r line; do
  LINE=$(printf '%s' "$line" | sed -E "s/'[^']*'//g; s/\"[^\"]*\"//g")
  if printf '%s' "$LINE" | grep -qE "${GIT}commit\b" &&
    printf '%s' "$LINE" | grep -qE '\s(--no-veri(fy?)?|-[a-zA-Z]*n[a-zA-Z]*)(\s|$)'; then
    deny "Do not skip git hooks with --no-verify or -n. If a hook refuses the commit, fix what it reports."
  fi
  # core.hooksPath set in any way: git -c, --config-env, GIT_CONFIG_KEY_<n>
  # or GIT_CONFIG_PARAMETERS, or git config other than reading it with --get,
  # --get-all or --list. A line that only mentions it, such as a commit
  # message's, goes through.
  if printf '%s' "$line" | grep -qi 'hookspath' &&
    { printf '%s' "$line" | grep -qiE -- "(-c\s*|--config-env[=[:space:]]+)['\"]?core\.hooksPath|GIT_CONFIG_(KEY_|PARAMETERS)" ||
      { printf '%s' "$line" | grep -qiE "${GIT}config\b.*core\.hooksPath" &&
        ! printf '%s' "$line" | grep -qE "${GIT}config\s+(--(get|get-all|list)|-l)\b"; }; }; then
    deny "Do not change core.hooksPath; it decides whether a repository's git hooks run."
  fi
done <<<"$JOINED"
exit 0
