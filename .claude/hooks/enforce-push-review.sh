#!/bin/bash
# Keeps Claude from skipping this repository's pre-push hook, which refuses a
# push to main that /review-loop hasn't found clean: no `git push --no-verify`.
# Registered only in this repository's own .claude/settings.json, so it
# applies to Claude working here and nowhere else.
#
# Every line of the command (lines a backslash continues joined first), with
# every quote and backslash taken out, since the shell takes them out before
# git sees the flag ("--no-verify", --no\-verify), is refused when git, push
# and --no-verify, or an abbreviation git takes for it (--no-veri, --no-verif;
# --no-ver is ambiguous with --no-verbose), are anywhere in it, whatever comes
# between (git -C "a path with spaces" push), git by its path too
# (/usr/bin/git). A push that names hooksPath, in any case (git -c
# 'core.hooksPath=/dev/null' push, GIT_CONFIG_KEY_0=core.hooksPath), is
# refused the same way, since it would switch the hook off. A line that only
# mentions them is refused too, which costs nothing.
#
# The record the hook reads is codex-review's to write (review-loop-clean,
# with review-loop-streak, the clean rounds leading to it), so a line that
# names either file and finds or writes it (git rev-parse --git-path, a
# redirection, tee, cp, mv, ln, rm, sed -i and the like) is refused; one that
# only mentions it, such as a commit message's, goes through. codex-review
# --recorded reads the record, and --record-squash moves it to a squashed
# commit.
#
# Without jq, or with JSON it can't read, it refuses rather than let the
# command through unread.
deny() {
  echo "$1" >&2
  exit 2
}
command -v jq >/dev/null 2>&1 || deny "This repository's push guard needs jq to read the command: install it."
INPUT=$(cat)
CMD=$(printf '%s' "$INPUT" | jq -r '.tool_input.command // empty') ||
  deny "This repository's push guard couldn't read the command it was given."
[[ -n "$CMD" ]] || exit 0
JOINED=$(printf '%s\n' "$CMD" | sed -e ':a' -e '/\\$/N; s/\\\n//; ta')
while IFS= read -r line; do
  BARE=$(printf '%s' "$line" | tr -d "\\\\'\"")
  if printf '%s' "$BARE" | grep -qE '(^|[^A-Za-z0-9_.-])git([^A-Za-z0-9_.-].*)?[^A-Za-z0-9_.-]push([^A-Za-z0-9_.-]|$)' &&
    { printf '%s' "$BARE" | grep -qE -- '--no-veri(fy?)?([^a-z-]|$)' || printf '%s' "$BARE" | grep -qi 'hookspath'; }; then
    deny "Do not skip git hooks, with --no-verify or core.hooksPath. This repository's pre-push hook wants a clean /review-loop first: run it."
  fi
  # Nor turning the hook's setting off: `switchboard setting review_before_push`
  # with a value or --reset after it. Reading it is fine.
  if printf '%s' "$BARE" | grep -qE 'setting[[:space:]]+review_before_push[[:space:]]+[^[:space:];|&]'; then
    deny "Do not change review_before_push: it is the user's to turn off, in Settings. This repository's pre-push hook wants a clean /review-loop first: run it."
  fi
  if printf '%s' "$BARE" | grep -qE 'review-loop-(clean|streak)' &&
    printf '%s' "$BARE" | grep -qE '>|--git-path|(^|[^A-Za-z0-9_.-])(tee|cp|mv|ln|rm|install|dd|truncate|touch|rsync|ditto|perl|python3?|node|ruby)([^A-Za-z0-9_.-]|$)|[[:space:]]-i'; then
    deny "Do not touch review-loop-clean or review-loop-streak: codex-review keeps them. Read the record with switchboard codex-review --recorded, and move it to a squashed commit with --record-squash <commit>."
  fi
done <<<"$JOINED"
exit 0
