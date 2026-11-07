# review-loop

A Claude Code skill that has Codex review the current branch, fixes what it
finds, commits, and reviews again until two consecutive rounds come back clean.
`SKILL.md` is the skill itself; this file is what you need to run it somewhere
else.

## Install

The skill is a Markdown file that tells Claude Code what to do. The work is
done by `switchboard codex-review`, a command of the `switchboard` binary, so
both have to be in place.

1. **Have the tools it runs.** The OpenAI Codex CLI, logged in with
   `codex login`; `codex exec` uses the same login as the interactive tool.
   Without Codex, Claude Code reviews in its place, headless, with only Read,
   Grep and Glob and no settings, MCP servers or CLAUDE.md. GitHub, signed in
   (`GH_TOKEN`, or the GitHub CLI's sign-in); the command uses it to fetch the
   PR title, body, and base branch so Codex never needs GitHub access of its
   own.

2. **Put `switchboard` on your `PATH`**, from Switchboard's releases. It
   carries `codex-review` and `skill-path`, which the skill runs first to find
   a repository's own `review-loop`:

   ```sh
   switchboard codex-review --help
   ```

   If the skill cannot find `switchboard codex-review` it stops and says so
   rather than reviewing on its own.

3. **Install the skill.** `switchboard setup plugin` writes it, with
   Switchboard's other skills, hooks and instructions, into a Claude Code
   plugin, `~/.local/share/switchboard/plugin`, which Claude in
   Switchboard's panes loads. Elsewhere, load it for a session with
   `claude --plugin-dir ~/.local/share/switchboard/plugin` (its skill is
   then `/switchboard:review-loop`, or `/review-loop` while nothing else has
   that name), or copy its `skills/review-loop` folder into
   `~/.claude/skills/`. With Settings' "Load Switchboard as a plugin" off,
   `switchboard setup skills` writes it into `~/.claude/skills/` for every
   session. Claude Code picks it up on the next session.

## What Codex is allowed to do

The command runs Codex with the read-only sandbox and no network for the
commands it runs, no approval path out of it, your Codex configuration and
approval rules ignored so no MCP servers or plugins load, `AGENTS.md` not
loaded as instructions, and only core environment variables visible to
commands. Contributor-written text such as the PR description is fenced in the
prompt as data, not instructions. No per-repository trust step is needed.

The sandbox does not confine reads to the repository; Codex cannot currently do
that without also making the repository writable. Findings that mention paths
outside the repository are flagged in the JSON's `warnings` array, and the
skill tells the runner to read those before acting.

## Using it

In a Claude Code session on a branch that is ready for review, type
`/review-loop`. Add `all` to also act on low-severity findings from the first
round. The skill commits per round, records disputed findings and accepted
known gaps in a notes file it passes back to Codex, stops after two consecutive
clean rounds or five rounds, and asks before pushing.

`switchboard codex-review` can also be run by hand for a one-off second
opinion. Run `switchboard codex-review --help` for the flags.
