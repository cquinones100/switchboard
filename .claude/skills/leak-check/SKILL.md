---
name: leak-check
description: Check what is about to reach Switchboard's public repository for anything identifying the user or their employer - switchboard leak-check on a branch's commits before its first push and before each commit pushed to an open PR, and on the text of a PR, an issue, a comment, a review or a release's notes before it is posted or edited - then a read by eye, and a question about anything left. For Switchboard's own repository. Use before any of those, whether a skill (queue-and-clean, review-loop, shape, diff-artifact) or the user starts it.
---

# Leak check

Switchboard's repository is public, and anything pushed or posted there stays public. Its `CLAUDE.md` ("Pushing") makes a check before each of those a rule of this repository. The skills Switchboard ships run no leak-check, since in someone else's repository the ticket IDs, PR numbers and employer's words it flags are how the work is written; so in this repository this skill is that step, taken where they push or post.

This is a project skill of Switchboard's repository (`.claude/skills/`), for working on Switchboard itself; the skills Switchboard ships are in `assets/skills/`, and this one isn't, so no Switchboard user gets it.

## When

- **Commits**: before a branch's first push (queue-and-clean's **Open a PR**, or any push that opens a PR), and before each commit pushed to a branch whose PR is open (a follow-up, a fix for a failing check, a rebase that resolved conflicts, on `origin/<base>..HEAD`), since the merge queue lands it as soon as CI passes.
- **Text**: before a PR is opened (its title and body) and before each edit of them (review-loop's step updating the PR body), before an issue is filed (shape's GitHub issue, its title and body) or edited (review-loop's ticket mode saving an issue), and before a comment, a review or its comments, or a release's notes is posted.
- **A page**: before diff-artifact publishes, on the branch's commits (`<merge-base>..HEAD` against its base) and on the page's words, which hold the hunks and the story.

## How

1. **Run it.** For commits, `switchboard leak-check <base>..HEAD`, `<base>` the base the commits go onto (`origin/main` after a fetch, or the PR's base). For text, write it to a file in the scratchpad and run `switchboard leak-check --file <file>`, or pipe it (`printf '%s' '<title>' | switchboard leak-check --file -`). It prints what it finds (the words in `~/.config/switchboard/leak-check.txt`, ticket IDs, PR numbers, home paths, email addresses, token shapes) and exits 1 when it finds any and 2 when it can't look. A hit on this repository's own issues and PRs (a `Closes #<issue>` line) is no leak, since they are public already.
2. **Read it by eye**, as the second check: organization or company names, ticket IDs, PR numbers, internal hosts, usernames, personal paths and tokens the script can't tell apart.
3. **Fix what you agree with**: in a commit, folded into it while it is unpushed; in text, in the file. Then run it again.
4. **Ask about the rest.** With anything left, or when it couldn't look, ask before anything is pushed or posted, with `AskUserQuestion` headed `[always ask]` (so autopilot's hands-free mode still asks it), showing what is left: **Push it as it is** (or **Post it as it is**) or **Not yet**. A question unanswered is a no. With nothing left, go on without asking.
