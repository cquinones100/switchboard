---
name: push-unreviewed
description: Get a push of this repository ready to go past the review rule (the pre-push hook that wants a clean /review-loop) - name the branch and its unpushed commits, run switchboard leak-check on them, read them for what identifies the user, then put the push command on the clipboard for the user to run themselves. For Switchboard's own repository. Use only when the user runs /push-unreviewed.
disable-model-invocation: true
---

# Push without the review

Switchboard's own skill for a push the user has decided needs no `/review-loop`. `.githooks/pre-push` refuses a push to `main` whose tip isn't the commit the review last found clean, and `.claude/hooks/enforce-push-review.sh` keeps every Claude here from going past it. This skill never goes past it either: it checks the commits as before any push, then hands the user the command, which they run in their own shell, where Claude's hooks don't apply.

This is a project skill of Switchboard's repository (`.claude/skills/`), for working on Switchboard itself; the skills Switchboard ships are in `assets/skills/`, and this one isn't, so no Switchboard user gets it.

Throughout: never run the push yourself, in any form; never change `.githooks/`, `.claude/hooks/`, `.claude/settings.json`, `core.hooksPath` or the `review_before_push` setting; never write the review's record. The only way this push happens is the user pasting it.

## 1. The branch and its commits

Run `git fetch origin`, then name the branch (`git branch --show-current`). When origin has the branch and `git merge-base --is-ancestor origin/<branch> HEAD` fails, it has moved past this one, and a push would be refused: bring the branch up to date first by reading the file `switchboard skill-path resolve-conflicts` prints and following it onto `origin/<branch>`, without its push, stopping to ask where it says to. Then name the commits it would push: `git log --oneline origin/<branch>..HEAD` when origin has the branch, else `git log --oneline origin/main..HEAD`. With none, say there is nothing to push and stop. That range is `<range>` below.

## 2. Leak check

Run `switchboard leak-check <range>` and show the user everything it prints. When it finds anything the user hasn't explained, stop there until they say to go on: the repository is public, and a pushed commit stays public.

## 3. Read them by eye

Read the range's added lines and commit messages (`git log -p <range>`) for what identifies the user or their employer that a script can't see: organization or company names, ticket IDs, PR numbers, internal hosts, usernames, personal paths, tokens. Say what you found, or that you found nothing, and stop on anything found until the user says to go on.

## 4. The command, on the clipboard

Write the command into a file in your scratchpad with the Write tool, one line with no newline at its end: `! git push` with git's flag that skips the hooks (`--no-verify`), then `origin` and the branch. Then run `pbcopy < <that file>`, and tell the user, in a line, that the push command is on their clipboard to paste into the prompt (the `!` runs it in their own shell) and which commits it pushes. Put it on the clipboard by the file, never by a Bash command that spells the flag out, which the repository's hook refuses.

Never run the push yourself, and don't wait on it or check for it: the user runs it when they choose. When the remote refuses it because the branch moved meanwhile, the user runs `/push-unreviewed` again, which brings the branch up to date and checks it once more.
