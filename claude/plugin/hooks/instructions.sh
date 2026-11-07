#!/usr/bin/env bash
# Switchboard's instructions for Claude (claude/switchboard.md: the tabs, the
# notes app, repository skills), handed to a session as it starts, resumes or
# comes back from a compaction, as SessionStart's additionalContext, since
# Claude Code reads no CLAUDE.md from a plugin. Only to an interactive
# session: a `claude -p` run (an entrypoint starting with sdk) gets nothing,
# so the story, connector and summary calls don't carry instructions they
# can't use.
case ${CLAUDE_CODE_ENTRYPOINT:-} in sdk*) exit 0 ;; esac
md="$(dirname "$0")/../../switchboard.md"
[[ -f $md ]] || exit 0
jq -Rs '{hookSpecificOutput: {hookEventName: "SessionStart", additionalContext: .}}' "$md"
