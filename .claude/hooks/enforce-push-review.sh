#!/bin/bash
# Keeps Claude from skipping this repository's pre-push hook, which refuses a
# push to main that /review-loop hasn't found clean: no `git push --no-verify`.
# Registered only in this repository's own .claude/settings.json, so it
# applies to Claude working here and nowhere else.
#
# The command's first line, with every quote and backslash taken out, since
# the shell takes them out before git sees the flag ("--no-verify",
# --no\-verify), is refused when git, push and --no-verify, or an abbreviation
# git takes for it (--no-veri, --no-verif; --no-ver is ambiguous with
# --no-verbose), are anywhere in it, whatever comes between (git -C "a path
# with spaces" push), git by its path too (/usr/bin/git). A push that names
# hooksPath, in any case (git -c 'core.hooksPath=/dev/null' push,
# GIT_CONFIG_KEY_0=core.hooksPath), is refused the same way, since it would
# switch the hook off. A line that only mentions them is refused too, which
# costs nothing.
INPUT=$(cat)
CMD=$(echo "$INPUT" | jq -r '.tool_input.command // empty')
[[ -n "$CMD" ]] || exit 0
BARE=$(echo "$CMD" | head -1 | tr -d "\\\\'\"")
if echo "$BARE" | grep -qE '(^|[^A-Za-z0-9_.-])git([^A-Za-z0-9_.-].*)?[^A-Za-z0-9_.-]push([^A-Za-z0-9_.-]|$)' &&
  { echo "$BARE" | grep -qE -- '--no-veri(fy?)?([^a-z-]|$)' || echo "$BARE" | grep -qi 'hookspath'; }; then
  echo "Do not skip git hooks, with --no-verify or core.hooksPath. This repository's pre-push hook wants a clean /review-loop first: run it." >&2
  exit 2
fi
exit 0
