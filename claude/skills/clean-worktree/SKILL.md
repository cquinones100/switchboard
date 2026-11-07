---
name: clean-worktree
description: Remove a git worktree and its local branch once nothing in them would be lost, with no uncommitted changes and every commit on GitHub, then prune the deleted remote branches. Queue-and-clean runs it once a PR has merged; run it yourself to clear away a worktree whose work is merged, closed or pushed.
argument-hint: "[worktree path or branch]"
disable-model-invocation: true
---

# Clean worktree

This is Switchboard's personal copy, installed for repositories that have none. Before anything else, run `skill-path clean-worktree`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Remove one worktree and its local branch, and nothing else: never the main clone, never the remote branch (GitHub or the repository's settings handle that), and never with `--force`. Run on its own, or from `/queue-and-clean`, which reads this file and follows it (see "From queue-and-clean").

When it is done, the final message is `clean-worktree: success` or `clean-worktree: fail: <reason>` and nothing else, the reason one line, with the error message when a command failed (`clean-worktree: fail: git worktree remove: '<path>' contains modified or untracked files`). The questions in step 2 are still asked, because they pause the work rather than end it.

Arguments: the worktree's path, or its branch. Otherwise the worktree you are in.

## Steps

1. **Find the paths.** From the worktree, `git rev-parse --show-toplevel` is the worktree, the parent of `git rev-parse --path-format=absolute --git-common-dir` is the main clone, and `git branch --show-current` the branch. Given a branch, find its worktree in `git worktree list --porcelain`. If the worktree and the main clone are the same directory, this is the main clone, not a worktree: say so and stop.

2. **Make sure nothing would be lost.** Stop and ask if either check fails, saying what would be lost. Never reach for `--force` to get past one.
   - `git -C <worktree> status --porcelain` must print nothing.
   - Every commit on the branch must be on GitHub. With a PR for the branch (`gh pr view <branch> --json state,headRefOid`, read again now, since a merge queue may have updated the branch), `git -C <worktree> rev-parse HEAD` must equal its `headRefOid`, merged or closed; a difference means local commits it doesn't have. With no PR, `git -C <main-clone> fetch origin`, then `git -C <worktree> branch -r --contains HEAD` must name at least one remote branch.

3. **Remove the worktree** from the main clone, so nothing runs from inside the directory being deleted:

   ```
   git -C <main-clone> worktree remove <worktree>
   ```

4. **Delete the local branch.** It has to be `-D`: a squash or rebase merge leaves the branch looking unmerged to git, and step 2 already showed GitHub has every commit on it.

   ```
   git -C <main-clone> branch -D <branch>
   ```

5. **Prune** with `git -C <main-clone> fetch --prune origin`, so a deleted remote branch stops showing up. Check nothing out and pull nothing in the main clone: its working tree is not yours to change.

If a command in steps 3 to 5 errors, stop there and reply `clean-worktree: fail: <command>: <its error message>`. Otherwise reply `clean-worktree: success`. If this session was in the worktree, its working directory is gone from here on, so every later command needs `git -C <main-clone>` or an absolute path.

## From queue-and-clean

`/queue-and-clean` records the paths in step 1 before it watches, while the worktree certainly exists, and follows steps 2 to 5 once the PR has merged and the user has approved its test, with the PR's number for step 2. It turns this skill's `fail` into its own, and closes out the ticket and note between steps 2 and 3, once step 2 has found nothing would be lost and before the worktree is removed.
