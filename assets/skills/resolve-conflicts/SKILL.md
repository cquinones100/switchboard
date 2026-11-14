---
name: resolve-conflicts
description: Bring the current branch up to date with the remote's main by fetching and rebasing onto it, then resolve any conflicts and carry the rebase, merge, or cherry-pick through to completion. Use when the user says they are in the middle of a rebase, mentions conflicts, says a PR has conflicts or is behind main, or when a git operation stops with conflicted files.
allowed-tools:
  - Bash(switchboard skill-path *)
  - Bash(git status)
  - Bash(git status *)
  - Bash(git log *)
  - Bash(git diff *)
  - Bash(git fetch)
  - Bash(git fetch *)
  - Bash(git rev-parse *)
  - Bash(git branch --show-current)
---

# Resolve conflicts

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path resolve-conflicts`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Bring the branch up to date with the remote's main, then resolve the conflicts in the git operation and drive it to completion. Work through every conflicting commit without stopping to ask.

## First: catch up with main

Only when no rebase, merge or cherry-pick is in progress (`git status` says so). If one is, skip straight to Resolving and finish it: never start a second operation on top of one.

- **Find the base.** Find the remote's default branch, falling back to `origin/main`, or `origin/master` if there is no `origin/main`:

   ```
   git symbolic-ref --short refs/remotes/origin/HEAD 2>/dev/null || echo origin/main
   ```

   Call it `<base>` (for example `origin/main`). If the current branch *is* that branch, there is nothing to rebase onto; say so and stop.
- **Check the tree is clean.** The working tree must be clean (`git status --porcelain` prints nothing). If it isn't, stop and ask: a rebase on top of uncommitted work risks it, and stashing it is the user's call.
- **Fetch,** so `<base>` is what the remote has now, not what was last fetched:

   ```
   git fetch origin
   ```

   Then record where the branch's own remote copy is, for the push at the end: `git rev-parse --verify --quiet origin/<branch>`, with `<branch>` from `git branch --show-current`. Nothing printed means the branch has never been pushed.

- **Stop if it is current.** If the branch already contains `<base>` (`git merge-base --is-ancestor <base> HEAD` succeeds), it is up to date: say so and stop, unless there are conflicts to resolve for some other reason.
- **Rebase.** Record the tip before rebasing (`git rev-parse HEAD`), then rebase onto `<base>` with the editor disabled:

   ```
   git -c core.editor=true rebase <base>
   ```

   Rebase onto the remote branch itself, not the local `main`: the local one may be behind, or checked out in another worktree where it can't be moved.
- **Then:** if it finishes without conflicts, go to step 7 of Resolving with the tip you recorded. If it stops on conflicts, work through Resolving from step 1; the tip you recorded is the pre-rebase tip step 7 compares against.

The branch now has new commits, so its remote copy needs a force push, which "Pushing" below does without asking.

## Resolving

1. Run `git status` to see which operation is in progress (rebase, merge, cherry-pick) and which files are conflicted. For a rebase, also record the pre-rebase tip now, while the operation is still in progress, so step 7 has something to compare against:

   ```
   cat "$(git rev-parse --git-path rebase-merge/orig-head)" 2>/dev/null || cat "$(git rev-parse --git-path rebase-apply/orig-head)" 2>/dev/null
   ```

   `--git-path` finds it in a worktree too, where `.git` is a file and the rebase state lives in the main repository's `.git/worktrees/<name>/`. Keep that SHA. Don't rely on `ORIG_HEAD` to still hold it later — it survives `--continue`, but any stash or reset along the way will overwrite it.
2. For each conflicted file, read it and resolve the conflict so both sides' intent is preserved. Use `git log` on the incoming commit when the intent isn't obvious from the markers alone.
3. `git add` each resolved file, then run `git diff --cached --check` to confirm no conflict markers were left behind.
4. Continue the operation with the editor disabled so it can't block on a commit message:
   - rebase: `git -c core.editor=true rebase --continue`
   - merge: `git -c core.editor=true merge --continue`
   - cherry-pick: `git -c core.editor=true cherry-pick --continue`
5. If the next commit also conflicts, repeat steps 2-4. Keep going until `git status` shows no operation in progress.
6. Report every commit that conflicted and how each one was resolved.
7. For a rebase, verify nothing was silently lost. Only once the rebase has fully finished — never between continues, where a detached mid-replay HEAD makes the output meaningless — run:

   ```
   git range-diff <orig-head-from-step-1>...HEAD
   ```

   Read the per-commit markers: `=` identical, `!` changed, `<` dropped, `>` added. Then check the output against what you expect:

   - A conflicted commit will show `!`, since it was replayed onto a new base. That's normal. What matters is the inner diff: lines the commit originally added should still be added. A `-+foo` paired with a `++bar` means your resolution replaced that commit's content, which is how intent gets dropped.
   - A `!` on a commit that never conflicted is suspicious and worth explaining.
   - A `<`/`>` pair for the same subject usually just means the content changed too much for `range-diff` to pair the commits, so inspect it rather than reporting it as a dropped commit.

   If the output shows content that should have survived but didn't, say so plainly and offer to fix it. Don't report the rebase as clean on the strength of step 3 alone — `git diff --cached --check` only catches leftover conflict markers, and a resolution that quietly drops a whole side passes it.

Steps 3 and 7 cover different failures, so run both. For a merge or cherry-pick, skip step 7: a merge doesn't rewrite commits (review the merge commit's diff instead), and cherry-pick never sets `orig-head`, so there is no before-tip to compare against.

Stop and ask instead of continuing when:

- A resolution requires choosing between two behaviors that can't both be kept, and the right choice isn't clear from the code or commit messages.
- The same commit conflicts again after a continue, which usually means the previous resolution was wrong.
- The only way forward is `--abort` or `--skip`, or dropping a commit's changes entirely.

Never `--abort` without asking first.

## Pushing

Once a rebase has finished and step 7 found nothing lost, force-push the branch to its own remote copy without asking: the user has said this skill may. Only with a lease on the remote tip recorded after fetching, so a push someone else made since is refused rather than overwritten:

```
git push --force-with-lease=<branch>:<recorded remote tip> origin HEAD:<branch>
```

Don't push, and say why, when:

- Step 7 showed content that should have survived but didn't, or anything else you asked about: the user decides first.
- The branch has no remote copy yet (nothing was recorded). A branch's first push is still asked for.
- The branch is `<base>`'s branch (`main`, `master`), or the operation was a merge or cherry-pick, which needs no force.
- The lease refuses the push: someone pushed to the branch meanwhile. Report it; never retry with `--force`.

Report the push as the user's CLAUDE.md asks of git operations: that it succeeded, and the new commit hash.
