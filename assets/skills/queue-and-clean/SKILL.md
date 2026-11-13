---
name: queue-and-clean
description: Queue the current branch's PR for merge if it is not queued already, watch it in the background, fix what stops it merging (failing checks, conflicts, being dropped from the merge queue), and once it merges wait for it to deploy, test the feature in a real browser and record a video of the test, and once you approve the test make sure its Linear ticket is done and its item in your notes ticked, then remove this worktree and its local branch. In a repository that lands straight on its default branch, as `switchboard landing` says (~/.config/switchboard/repos.json, else GitHub's rules and the repository's CLAUDE.md), it lands the branch there instead, with the steps it gives, one worktree at a time.
argument-hint: "[PR number]"
disable-model-invocation: true
---

# Queue and clean

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path queue-and-clean`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Queue this worktree's PR for merge, in the merge queue or with auto-merge, unless it is already queued. Then watch the PR without blocking the conversation. When something the branch can fix stops it merging (a required check fails, it has merge conflicts, or the merge queue drops it), fix it, push, queue it again and go back to watching, as described in "Fixing". When it merges, wait until the merge has deployed, test the feature on the deployed site in a real browser, recording a video of the test, and ask the user to approve it ("Deploy and test"). Once they approve, remove the worktree and its local branch. If it closes without merging, or stops for something you can't fix, leave everything as it is.

When the work is done, the final message is either `queue-and-clean: success` or `queue-and-clean: fail: <reason>` and nothing else: no summary of what was checked or what was removed. `success` means the PR merged, the user approved the test (or chose to clean up without one) and the cleanup finished; if you fixed anything on the way, say what in a few words after it, as `queue-and-clean: success (fixed: <what>)`, for example `queue-and-clean: success (fixed: lint error in api.ts; reran a flaky e2e job)`. What "Deploy and test" and "Closing out" did goes in the same brackets, after anything fixed: `queue-and-clean: success (tested: <video>; ticket ABC-123 done)`. `fail` means the PR could not be queued, closed without merging, failed to deploy, had its test turned down, stopped for something "Fixing" leaves alone or still failed after its last round, or a cleanup command errored, and the reason is one line saying which, with the error message when a command failed (for example `queue-and-clean: fail: git worktree remove: '<path>' contains modified or untracked files`), and what you tried when you tried fixing it. The questions in "Before watching", the merge method question in "Queuing", the questions in "Fixing", the questions and the approval in "Deploy and test", the questions in clean-worktree's step 2 and the one in "Applying the change" are still asked, because they pause the work rather than end it.

Arguments: a PR number, if one was given. Otherwise use the current branch's PR; when the branch has none, follow "No PR" instead.

## Keeping the tab's name

First of all, before "Before watching" or "No PR", and only when `$SWITCHBOARD_PANE` is set: pin the tab's title as its name, while it still comes from the worktree's branch (its ticket and PR). Once the worktree is gone and the pane restarts in the main clone, on its default branch, the title would change to the repository's name; a name of its own wins over the branch. When `switchboard show @tab_name` prints nothing, run:

```
switchboard set @tab_name "$(switchboard show @tab_label | sed -E 's/^#[0-9]+ //')"
```

The `sed` takes off the `#<PR> ` the label starts with when the branch has a PR, which the tab puts in front of its name itself, so it isn't shown twice. Skip it when `@tab_label` is empty. A tab with a name of its own already (`/wtn`'s, autopilot's, one you gave it) keeps it, and a full title it keeps in `@tab_title` is left as it is.

## Before watching

1. Read the PR and the repo:

   ```
   gh pr view <number-if-given> --json number,url,state,isDraft,headRefName,headRefOid,autoMergeRequest
   gh repo view --json owner,name --jq '.owner.login + " " + .name'
   ```

2. Record the paths the cleanup needs now, while the worktree still exists, as step 1 of the file `switchboard skill-path clean-worktree` prints finds them:

   ```
   git rev-parse --show-toplevel
   git rev-parse --path-format=absolute --git-common-dir
   ```

   The first is the worktree. The parent of the second is the main clone. If they are the same directory this is the main clone, not a worktree: say so and stop.

3. Stop and ask when any of these is true:
   - The PR's `headRefName` is not the branch checked out here. Cleaning up would remove the wrong thing.
   - The PR is `CLOSED`.
   - The PR is a draft. Queuing it would mean marking it ready for review, which is the user's call.

   If the PR is already `MERGED`, skip the queue and the watch and go straight to "Deploy and test".

4. Settle the ticket and the note now, so finishing up later asks nothing:
   - **The ticket**: the Linear ticket the branch names (an ID like `ABC-123` at the start of the branch name), else one the PR's title or body names. None is fine: then there is no ticket to close out.
   - **The note**: which of the user's notes holds this work's checklist item, settled as `~/.local/share/switchboard/claude/plugin/skills/note-watch/pick-note.md` says, with **No note** as an option, since not every PR has one. For a branch `/wtn` opened, that takes the note its prompt names without asking, as pick-note's step 2 says. Remember the answer; read the note only at "Closing out".
   - **The deploy and the test**: settle both as "Settling" in the file `switchboard skill-path deploy-test` prints says, so nothing after the merge has to ask.

## No PR

When no argument was given and `gh pr view` finds no PR for the branch (nor a closed or merged one), the work may have reached the default branch some other way (merged by hand, cherry-picked, squashed in with other work), or not at all. Record the paths as step 2 of "Before watching" says, then run `switchboard landing --json <worktree>`: when its `target` isn't null (a branch cut from another branch to land there, as "A target branch" below says), that branch stands for the default branch in all of steps 1 to 3, as that paragraph resolves it: `<default>` is `<remote>/<remoteBranch>`, fetched from `<remote>`, when it tracks one, else the local `<name>` (`refs/heads/<name>`), nothing fetched; its name in place of the default branch's wherever these steps say it, so work already in the default branch but not in the target is not taken as landed. Then:

1. **Is it all in the default branch?** (or the target, as just said) Find it and fetch it: its name is what GitHub calls it now for origin's repository, `gh repo view "$(git -C <main-clone> remote get-url origin)" --json defaultBranchRef --jq .defaultBranchRef.name` (origin's, not whichever repository gh would pick, a fork's upstream say), since a clone's own `origin/HEAD` can be out of date (when gh can't say, ask), and `git -C <main-clone> fetch origin "+refs/heads/<name>:refs/remotes/origin/<name>"` must succeed, a clone that fetches only some branches included; `<default>` below is `origin/<name>`, then:
   - `git -C <worktree> status --porcelain` prints nothing: uncommitted work is in no branch, so ask about it as clean-worktree's step 2 would.
   - Merging the branch into the default branch changes nothing: `git -C <worktree> merge-tree --write-tree <default> HEAD` succeeds and prints the same tree as `git -C <worktree> rev-parse <default>^{tree}`. That settles it whichever way the work got there, a squash included. When it doesn't (a conflict, or a different tree), the work may still be there, squashed in with changes beside it: read what the branch changed (`git -C <worktree> diff <merge-base> HEAD`, the merge base from `git merge-base <default> HEAD`) against the default branch's version of those files, and count it as in when every change is there; when you can't tell, ask, showing what looks missing.
2. **When it is**, there is nothing to queue, watch or test: settle the ticket and the note as step 4 of "Before watching" says (no deploy, no test), then follow "Cleanup", which closes them out ("Closing out") once nothing would be lost and before the worktree goes, with this one change, that clean-worktree's step 2 counts the work as on GitHub because the default branch holds it, its own check of the branch's commits on a remote skipped. Then "Back to the repository" as after a merge, the success line saying so: `queue-and-clean: success (no PR: already in <default>; …)`.
3. **When it isn't**, say what the default branch lacks (`git -C <worktree> log --oneline <default>..HEAD`, and the files of `git -C <worktree> diff --stat <default>...HEAD`), and run `switchboard landing <worktree>` to learn how the repository lands (its first line `direct` or `pr`, from `~/.config/switchboard/repos.json`, else GitHub's rules and the repository's `CLAUDE.md`). For a branch `/wtn` opened, told as pick-note's step 2 tells one, where `landing` prints `direct`, its prompt has settled that it lands there: follow "Land on the default branch" below without asking. Otherwise ask with `AskUserQuestion`:
   - **Land on <default's name>**, only when `landing` prints `direct`, and then first, as recommended: follow "Land on the default branch" below.
   - **Open a PR** (first, as recommended, when it prints `pr`): push the branch, its first push, and open the PR with `gh pr create` as the repository's own PR conventions say, then carry on from "Before watching" with it.
   - **I'll merge it myself**: reply `queue-and-clean: fail: no PR, and <default> lacks <n> commits`, changing nothing; merging into the default branch is the user's to do.
   - **Leave it**: the same reply, changing nothing.

### Land on the default branch

For a repository `landing` says lands straight on its default branch: the branch's work goes onto it, tested, reviewed and pushed, as the repository's steps say. They are `landing`'s second line, JSON (`switchboard landing --json` gives them with the method): `test`, the command that runs its tests (empty, none; `null`, not set); `testIn`, where they run, `here` (in the worktree) or `ci` ("The tests in CI" below); `squash`, whether it lands as one commit; `review`, `review-loop` or `none`; `beforePush`, a command that must pass before the push (empty, none); `afterRebase`, a command step 9 runs in place of the tests after a clean rebase (empty, the tests run again); and `source`, where the method came from (`repos.json` when the user set it). Everything up to the push runs in each worktree at once; only the push itself, with the rebase and tests just before it, waits for the others, one worktree at a time behind the landing lock of the branch it lands on. `<lock>` below is `switchboard land-lock`. The tab shows which step the landing is at: run `<lock> phase <words>` as each step below that names a phase starts, and `<lock> phase` alone, clearing it, before the final line, whichever way the section ends. The Landing view also shows what the landing has done, from what `<lock> record` keeps in the worktree: run `<lock> record clear` as step 1 starts; each time the tests run (steps 5 and 9), save their output to a file in the scratchpad and run `<lock> record tests '{"ok":<true when they passed, else false>,"passed":<n>,"failed":<n>,"total":<n>,"file":"<the first failing test file, or empty>"}'` with the counts the run printed (from CI, the failed jobs' names in place of the file, with the counts left out when the log gives none, `ok` still saying whether they passed), then `<lock> record output tests <that file>`; and after each `leak-check`, `<lock> record leak '{"found":<how many it printed>}'`. A record failing never stops the landing.

**A target branch.** `landing`'s JSON has `target`: null for the default branch, which the steps below are written for; else the branch autopilot, "Start work on a note's checklist items" or `/wtn` cut this one from (`wtn --target`, kept as `branch.<branch>.wtnTarget`), which the landing goes onto instead of the default branch, the same steps with these changes. `<name>` is the target's `branch` wherever it names the branch landed on. When the target tracks a remote branch (`remote` and `remoteBranch` set), every `origin` and `origin/<name>` below is `<remote>` and `<remote>/<remoteBranch>`, the push is `git push <remote> HEAD:<remoteBranch>`, and step 12 fast-forwards the local `<name>` too, `git -C <main-clone> merge --ff-only <remote>/<remoteBranch>` when the main clone has it checked out, else `git -C <main-clone> fetch . <remote>/<remoteBranch>:<name>`. When it tracks none (`remote` null), the landing only moves the local branch: nothing is fetched, `origin/<name>` below is the local `<name>` (`refs/heads/<name>`), and step 11's push is a fast-forward of it, `git -C <main-clone> merge --ff-only <the commit>` when the main clone has `<name>` checked out, else `git push . HEAD:<name>` from the worktree (never forced), with step 12's fetch and merge left out. The success line names the target: `landed on <name>: <short hash>`. The landing lock and its queue are the target's own, which `<lock>` finds from `wtnTarget` with no flag, so worktrees landing on different branches never wait on each other. A target never takes the default branch's work through a landing: `land-lock landed` after a direct landing on the default branch opens a tab running `switchboard update-target <branch>` for each branch items land on (Settings' `target_follows_default`), which merges the default branch into it under its own lock, so no step of a landing does that.

**The tests in CI.** With `testIn` `ci`, steps 5 and 9 run the tests in the repository's CI rather than in the worktree, on a branch of this landing's own, `ci/land-<id>`, `<id>` being what `git rev-parse --abbrev-ref HEAD | git hash-object --stdin | cut -c1-12` prints: the worktree's branch name as a hash, the same at every push of this landing, since the name itself (a note item's words, for a branch `/wtn` opened) is never published before step 7's question, so the computer runs none of them; `test` is then only what runs here when no CI answers. Each time a step has the tests to run:

- **Check it first**: `switchboard leak-check <base>..HEAD`, as step 7 runs it, since the commit leaves the computer here, before step 7's question. A hit you agree with is fixed and folded into the commit; with any left, or when it can't look (exit 2), ask before anything is pushed, with `AskUserQuestion` headed `[always ask]`, showing them: **Push to ci/land-<id>** or **Run them here**, which runs the tests in the worktree as `here` does.
- **Push it**: `git push --force origin HEAD:refs/heads/ci/land-<id>`, forced since each fix rewrites the commit and the branch is this landing's own, never another branch and never `<name>`. `testIn` is the standing answer for this push, so nothing asks; a push refused or failing is said in a line, and the tests run here.
- **Wait for it** (phase `testing in CI`): `switchboard wait-ci --repo <owner/name> --sha "$(git rev-parse HEAD)"` in the background (`run_in_background`), `<owner/name>` being the repository the commit was pushed to (`landing`'s `repo` for `origin`; for another remote, `gh repo view "$(git remote get-url <remote>)" --json nameWithOwner --jq .nameWithOwner`), which the `gh run view` commands below name too with `-R <owner/name>`, and go on once it prints its line:
  - `passed <sha>`: the tests passed, and count as run for step 7.
  - `failed <sha> <run id> <job names>`: the tests failed. Read why with `gh run view <run id> --log-failed` (a run id of `-` is a check of another CI than GitHub Actions: read its failure where that CI shows it, or run the tests here to see it), which answers once the run has ended; while its other jobs still run, `gh run view <run id> --json jobs` gives each failed job's `databaseId` for `gh run view --job <id> --log-failed`. Then, before fixing anything, ask whether the default branch fails the same way: `switchboard wait-ci --now --repo <owner/name> --sha "$(git rev-parse origin/<name>)"` (after `git fetch origin <name>`). When it prints `failed <sha> <run id> …` too, read that run's failures the same way, and when every test the landing's run failed also failed there, the failure is the default branch's, not the change's: run `<lock> phase waiting for <name> to pass`, run `<lock> release` if this worktree holds the lock, and wait in the background (`run_in_background`) with `switchboard wait-ci --repo <owner/name> --sha <tip>` on `origin/<name>`'s tip, fetching again for each new tip until one prints `passed`, then go back to step 3, fixing nothing. A test that failed only on the landing's run is the change's: the step goes on as it does when its tests fail, a fix pushed to the same branch and waited for the same way.
  - `none`: no CI ran on the branch, or every job it has was skipped there. Say so in a line and run the tests here, as `here` does, for the rest of this landing.
  - `timeout`: a failure, `queue-and-clean: fail: timeout waiting for CI on ci/land-<id>`.
- **Delete the branch** on every way out of this section, with `git push origin --delete ci/land-<id>`, once one of these pushes has succeeded: after step 11's push, at **Not yet**, and before a failure's final line alike. A branch already gone is fine.

With a target branch that tracks a remote one, `origin` here is its `<remote>`; one that tracks none has nowhere to push, so its tests run here.

1. **The note.** Settle it now, as step 4 of "Before watching" says for the note alone: no ticket, deploy or test, since the push is the whole landing. For a branch `/wtn` opened, that is the note its prompt names, taken without asking.
2. **Clean tree.** `git status --porcelain` prints nothing, or ask about what is uncommitted as clean-worktree's step 2 would.
3. **Onto the default branch as it is now** (phase `rebasing`): `git fetch origin <name>`, then note the commit it is at, `base=$(git rev-parse origin/<name>)`, and `git rebase "$base"`. Use that `$base`, never `origin/<name>`, until step 9: `origin/<name>` is shared by every worktree of the repository, so another one's fetch moves it while this one works, which step 9 takes in. On a conflict, resolve it as `/resolve-conflicts` does (read the file `switchboard skill-path resolve-conflicts` prints, without its push), and stop and ask when the right resolution isn't clear, with `AskUserQuestion` headed `[always ask]` (so autopilot's hands-free mode still asks it).
4. **One commit** (phase `squashing`), only with `squash` on; off, the branch's commits land as they are, and "the commit" below means them all. A `backup/pre-<what>` branch first, named for this branch, since other worktrees squash at the same time, then `git reset --soft "$base"` and one commit whose message describes the feature as it now stands, written as the repository's `CLAUDE.md` says for commit messages, and check the tree equals the backup's. Where the repository's `CLAUDE.md` asks for it before a push (a public repository's may), check the added lines and the message for anything identifying.
5. **The tests pass before the review** (phase `testing`, named only when there are tests to run, since the board takes it as tests run): with `testIn` `ci`, run them as "The tests in CI" says; otherwise run `test` in the worktree. When it is empty, there are none to run; when it is `null` (not set), run the tests the repository's `CLAUDE.md` names, if any. When they fail, fix what the branch broke and fold the fix into the commit, or stop and ask when it isn't the branch's, with `AskUserQuestion` headed `[always ask]`.
6. **Review it** (phase `reviewing`), only with `review` `review-loop`; with `none`, go on to step 7. Read the file `switchboard skill-path review-loop` prints and follow it on this branch up to its final report, its push question excepted: the push is step 7's. In a tab that lands without asking (step 7's `@autopilot_land`), the loop fixes the medium findings it agrees with without asking, as its own rules say for such a tab. With `squash` on, fold the loop's fix commits into the one commit as its squash steps say, recording the clean review for the squashed commit (`review-loop-clean`: the commit, its base and its `patchId`). Whether it squashes or not, run the tests again, as step 5 does, when the loop changed anything.
7. **Ask before pushing**. First run `switchboard leak-check <base>..HEAD`, `<base>` being the base the commit is on now (`$base`, or `origin/<name>` once step 9 has rebased onto it): it prints anything in the added lines and commit messages that may identify the user or their employer (the words in `~/.config/switchboard/leak-check.txt`, ticket IDs, PR numbers, home paths, email addresses, tokens), and exits 1 when it finds any and 2 when it can't look; a hit you agree with is fixed and folded into the commit, going back to step 5, and the rest are shown in the question. Then, when `switchboard show @autopilot_land` prints `1` (a tab autopilot opened with its "Land without asking" on), `leak-check` exited 0, the landing ran both the tests and the review loop (`review` is `review-loop`, and step 5 had tests to run: `test` not empty, or not set and the repository's `CLAUDE.md` naming them), and the loop ended on two clean rounds of this very change (`review-loop-clean`'s third field, `git rev-parse --git-path review-loop-clean`, equal to `switchboard codex-review --patch-id <base> HEAD`), take the answer as **Push to <name>** without asking, saying so in a line (`Landing without asking: <subject>`); anything else asks, after `<lock> phase asking to land` (so the board's review time stops where the question starts): with `AskUserQuestion`, its header `[always ask]` (so a tab in autopilot's hands-free mode still asks it), **Push to <name>** or **Not yet**, the commit's subject in the question. The answer covers this change landing as reviewed: steps 8 to 10 push it without asking again only while it stays the same change, and ask again when it doesn't. With `review` `none`, note the change asked about now for step 10: `switchboard codex-review --patch-id "$base" HEAD` here, and `switchboard codex-review --patch-id origin/<name> HEAD` when step 10 asks again, against the base it is on by then.
8. **Take the landing lock**: `<lock> take`. It exits 3 when another worktree is landing or others wait ahead of this one, printing which, and puts this worktree in the landing queue: say so in a line, run `<lock> phase waiting for the landing lock`, then `<lock> wait` in the background (`run_in_background`), which takes the lock once this worktree's turn in the queue comes, and go on once it finishes. The sidebar and mission control's Landing view show the queue, where the user can move a waiting tab up or down, or take it out: then `wait` exits 4 ("taken out of the landing queue"), and the landing stops as **Not yet** does, the reply saying it was taken out of the queue. From here, every way out of this section, a failure included, stops that `wait` if it still runs and runs `<lock> release`, which also takes this worktree out of the queue, before the final line.
9. **Onto the default branch as it is now, again** (phase `rebasing again`): `git fetch origin <name>`. When `origin/<name>` hasn't moved past this commit's base (`git merge-base --is-ancestor origin/<name> HEAD` succeeds), the commit is the one step 5 tested: go on to step 10 with nothing run. When it has, `git rebase origin/<name>`, a conflict resolved as in step 3. Then, when `afterRebase` is set, the rebase had no conflict, and `switchboard wait-ci --now --repo <owner/name> --sha "$(git rev-parse origin/<name>)"` answers and prints anything but `failed …` (its exit 1, GitHub not answering, says nothing of the default branch, so the tests run), check the rebase rather than test it again: run `<lock> phase checking after the rebase` and `afterRebase` in the worktree, the lock held. The tests passed on the change at step 5 and on the default branch in its own CI, and a clean rebase only puts the two side by side, so the check is for what doesn't build or lint together. It passing goes on to step 10. Otherwise (a conflict, no `afterRebase`, the default branch failing, or no answer about it) run `<lock> phase testing again` and the tests again, as step 5 does, in CI with `testIn` `ci`, the lock held while it runs. When the tests or the check fail, the change and what landed meanwhile break each other: run `<lock> phase tests failed <how many of how many, as "2 of 401">` (`<lock> phase check failed` for the check), which the Landing view shows as a failed cell, then release the lock, fix it, fold the fix into the commit, and go back to step 5.
10. **Still the change the review found clean?** With `review` `none`, there is no review to compare with: compare `switchboard codex-review --patch-id origin/<name> HEAD` with the hash noted at step 7 instead, going on to step 11 when they are the same and the rebase had no conflict, and otherwise asking again as step 7 does, holding the lock (**Not yet** releasing it), since the change asked about is no longer the one being pushed. With `review-loop`: `switchboard codex-review --patch-id origin/<name> HEAD` prints the change from the default branch to the commit as one hash; compare it with `review-loop-clean`'s third field (`git rev-parse --git-path review-loop-clean`). The same means the commit is the reviewed change on a newer base, its context lines included, so nothing landed beside it: go on to step 11 with no second review, which would only sample the reviewer again (the tests or the check in step 9 are what shows that what landed elsewhere still works with it). Different, or the rebase had a conflict: review it again as step 6 does, holding the lock (phase `reviewing again`; phase `review blocked` when it ends with findings waiting on the user), then ask again as step 7 does (**Not yet** releasing the lock), since the change asked about is no longer the one being pushed.
11. **Push** (phase `pushing`): check the lock is still this worktree's (`<lock> take` exits 0; when it doesn't, another landing took it over, so go back to step 8), and fetch again: when `origin/<name>` has moved, go back to step 9. With `beforePush` set, run it in the worktree now: when it fails, that is a failure, saying what it printed. When step 7 took the answer without asking, read `@autopilot_land` again as it did: it no longer printing `1` means "Ask before landing" was chosen meanwhile, so ask as step 7 does, holding the lock (**Not yet** releasing it). Then `git push origin HEAD:<name>`, never forced, run in the worktree as a command of its own written exactly so (no `-C`, nothing chained to it): a tab of autopilot's has that one command allowed in Claude Code's auto mode, which may refuse a push written any other way. A pre-push hook refusing it (one may check the review, taking the same change rebased by its `patchId` as step 10 does, as Switchboard's does) is a failure: say what it said. Once it has pushed, asked about or not, run `switchboard set @autopilot_pushed <short hash>` (it marks a tab of autopilot as landed; elsewhere it is harmless) and `<lock> landed <short hash> <name>` (it files the tab under Merged, as landed on `<name>`), delete the landing's `ci/land-<id>` when "The tests in CI" pushed one, and, when step 7 took the answer without asking, say so at once, before anything after can fail or end the turn (a hands-free tab is sent back on a question until the tab is marked landed): `<lock> autolanded <short hash> [--plan <the item's plan note>] <the item's first line>` (an item with no plan passing `--decided <n>` instead, the count of `Decided hands-free` lines indented under it in the note, when it has any), which puts "Landed <short hash>: <item>" in the notifications, with the count of the plan's `Decided hands-free` lines and a way to open it, and marks the tab (with Settings' "Notify when autopilot lands" off, the server keeps it in the notifications' history alone, as dismissed), and, only when `switchboard setting autopilot_landing_notices` prints `on`, `switchboard slack-relay notify --from landing "Landed <short hash> on <name> without asking: <item>"`, whose failure (the relay not running) is fine.
12. **The main clone**: note where it is, `before=$(git -C <main-clone> rev-parse HEAD)`, then `git -C <main-clone> fetch origin <name>`, and, when the main clone has `<name>` checked out, `git -C <main-clone> merge --ff-only origin/<name>`, which runs the main clone's post-merge hook where it has one. When it can't fast-forward (commits of the main clone's own not yet pushed, or uncommitted changes in the same files), leave it and say so in the final line.
13. **Release the lock**: `<lock> release`. Then, when step 12 moved the main clone, follow "Applying the change" for `$before..HEAD` (phase `applying the change`).
14. **Then as work already in the default branch** (phase `cleaning up`): carry on as step 2 above does, "Cleanup" (its "Closing out" before the worktree goes) and "Back to the repository", the note's item ticked with a `Done: <short hash>, <took>. <what the feature does now, in prose>` line under it when the note's other ticked items carry one (`<took>` as "Closing out" says), `Done: <short hash>, <took>, landed without asking. …` when step 7 took the answer without asking, and, when the item's plan, or the item itself when it has none, holds `Decided hands-free` lines (autopilot's hands-free mode), a sentence ending the line that names what was decided. The success line says so: `queue-and-clean: success (landed on <name>: <short hash>; applied: built and installed Switchboard; note: ticked "…")`.

**Not yet**: run `<lock> release` (it lets the lock go only if this worktree holds it), delete the landing's `ci/land-<id>` when "The tests in CI" pushed one, clear the phase, and reply `queue-and-clean: fail: not pushed; the branch is ready to land`, keeping the worktree.

## Queuing

Check whether the PR is already queued, with the same query the watch uses:

```
gh api graphql -F owner=<owner> -F repo=<repo> -F number=<number> \
  -f query='query($owner: String!, $repo: String!, $number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $number) { autoMergeRequest { enabledAt } mergeQueueEntry { state } } } }' \
  --jq '.data.repository.pullRequest | if .autoMergeRequest or .mergeQueueEntry then "queued" else "unqueued" end'
```

If it prints `queued`, skip the rest of this section and start watching. If it prints `unqueued`, read how the repo merges before trying anything, since that decides the command:

```
gh api graphql -F owner=<owner> -F repo=<repo> -F base=<baseRefName> \
  -f query='query($owner: String!, $repo: String!, $base: String!) { repository(owner: $owner, name: $repo) { autoMergeAllowed mergeQueue(branch: $base) { id } } }' \
  --jq '.data.repository | "auto-merge=\(.autoMergeAllowed) queue=\(.mergeQueue != null)"'
```

(`baseRefName` comes from `gh pr view <number> --json baseRefName`.) Then:

- **Auto-merge allowed** (`auto-merge=true`): `gh pr merge <number> --auto --match-head-commit <headRefOid>`. GitHub queues or merges the PR itself once its checks pass. Then start watching.
- **Auto-merge off, with a merge queue** (`auto-merge=false queue=true`): wait until the PR is ready (below), then add it to the queue directly. Don't use `gh pr merge` here. gh chooses between queuing and auto-merge from GitHub's `mergeStateStatus`, which GitHub works out lazily and often reports as `UNKNOWN` or `BLOCKED` for a PR that can be queued, so gh asks for auto-merge and the repo refuses.
- **Auto-merge off, no merge queue** (`auto-merge=false queue=false`): wait until the PR is ready (below), then `gh pr merge <number> --match-head-commit <headRefOid>` without `--auto`.

`--match-head-commit` (and `expectedHeadOid` below) make GitHub refuse if the branch moved since you read it, so what gets queued is what was checked. Without a merge queue, gh needs a merge method: read the ones the repo allows,

```
gh repo view --json squashMergeAllowed,mergeCommitAllowed,rebaseMergeAllowed
```

and if exactly one is allowed, pass its flag (`--squash`, `--merge`, or `--rebase`). If more than one is, ask the user which to use. Keep that flag for any later `gh pr merge` in this run. Never pass `--admin`, which skips the queue and the required checks, and never pass `--delete-branch`.

### Waiting until the PR is ready

When the repo doesn't allow auto-merge, do its job: wait until the required checks have passed and the PR has no conflicts, judged from the checks themselves rather than `mergeStateStatus`. Start this with the Bash tool's `run_in_background`, as with the watch below, so the conversation is not blocked:

```
settled=0
while :; do
  info=$(gh pr view <number> --json state,mergeable --jq '[.state, .mergeable] | @tsv') || { sleep 30; continue; }
  IFS=$'\t' read -r state mergeable <<<"$info"
  case "$state" in MERGED) echo merged; exit 0 ;; CLOSED) echo closed; exit 0 ;; esac
  [ "$mergeable" = CONFLICTING ] && { echo conflicts; exit 0; }
  # Each required check's bucket. gh exits non-zero while checks fail or are
  # pending, so only no output at all is an error; a repo with no required
  # checks says so on stderr.
  out=$(gh pr checks <number> --required --json bucket --jq 'if length == 0 then "none" else ([.[].bucket] | unique | join(" ")) end' 2>&1)
  case "$out" in *"no required checks"*) out=none ;; esac
  [ -z "$out" ] && { settled=0; sleep 30; continue; }
  case " $out " in *" fail "*|*" cancel "*) echo failed; exit 0 ;; esac
  case " $out " in
    *" pending "*) settled=0 ;;
    *) settled=$((settled + 1)); [ "$settled" -ge 2 ] && { echo ready; exit 0; } ;;
  esac
  sleep 30
done
```

Ready has to hold twice in a row, a minute apart, because a required check can be created late. A failed request is retried rather than read as an answer.

When it exits:

- `ready`: with a merge queue, add the PR to it. `expectedHeadOid` does what `--match-head-commit` does:

  ```
  gh api graphql -F id="$(gh pr view <number> --json id --jq .id)" -F oid=<headRefOid> \
    -f query='mutation($id: ID!, $oid: GitObjectID!) { enqueuePullRequest(input: {pullRequestId: $id, expectedHeadOid: $oid}) { mergeQueueEntry { position state } } }'
  ```

  The user's `enforce-gh-api-readonly.sh` hook blocks every other GraphQL mutation and lets this one through on its own, so send it alone. Never call `enablePullRequestAutoMerge`: the repo has turned auto-merge off on purpose. Without a merge queue, run `gh pr merge` as above. Then start watching.

  If GitHub refuses because a required check hasn't passed after all (one that appeared late), start the wait again, up to three times in all. If it refuses because of a required review or anything else that isn't a check, reply `queue-and-clean: fail: <GitHub's error message>`. If the branch moved while you waited, the head check makes it refuse; that is the right outcome, since the new commits are not the ones that were checked: report it as any other failure.
- `failed`: required checks failed. Go to "Fixing".
- `conflicts`: the PR has merge conflicts. Go to "Fixing".
- `merged` or `closed`: someone else finished it. Go to "Deploy and test" for `merged`; reply `queue-and-clean: fail: PR closed without merging` for `closed`.

### Any other failure

If `gh pr merge` or the queue mutation fails because of a merge conflict or a failing required check, go to "Fixing". If it fails for any other reason, such as a missing review, reply `queue-and-clean: fail: <the command>: <its error message>` and stop.

Once it succeeds, start watching. Do not check the state yourself first: right after queuing, GitHub can briefly show neither auto-merge nor a queue entry, and the watch already allows for that. It also covers a PR that GitHub merged straight away because nothing was left to wait for.

## Watching

Start this with the Bash tool's `run_in_background`, filling in owner, repo, and number. You are re-invoked when it exits, so do not poll it yourself and do not sleep in the foreground. Tell the user that you are watching and that they can keep working in the meantime.

```
query='query($owner: String!, $repo: String!, $number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $number) { state autoMergeRequest { enabledAt } mergeQueueEntry { state } } } }'
misses=0
while :; do
  pr_state=$(gh api graphql -F owner=<owner> -F repo=<repo> -F number=<number> -f query="$query" \
    --jq '.data.repository.pullRequest | [.state, (if .autoMergeRequest or .mergeQueueEntry then "queued" else "unqueued" end)] | @tsv') || { sleep 60; continue; }
  case "$pr_state" in
    MERGED*) echo merged; exit 0 ;;
    CLOSED*) echo closed; exit 0 ;;
    *unqueued) misses=$((misses + 1)); [ "$misses" -ge 2 ] && { echo dequeued; exit 0; } ;;
    *) misses=0 ;;
  esac
  # With auto-merge, a failed required check leaves the PR waiting forever.
  if gh pr checks <number> --required 2>/dev/null | awk -F'\t' '$2 == "fail"' | grep -q .; then
    echo failed; exit 0
  fi
  sleep 60
done
```

A failed request is retried rather than read as a result, so a network blip never ends the watch. Leaving the queue has to be seen twice in a row, because GitHub briefly shows neither auto-merge nor a queue entry while it moves a PR from one to the other.

When it exits:

- `merged`: go to "Deploy and test".
- `closed`: leave the worktree alone and reply `queue-and-clean: fail: PR closed without merging`.
- `failed`: a required check failed while auto-merge waited. Go to "Fixing".
- `dequeued`: find out why the PR left the queue. GitHub records it:

  ```
  gh api graphql -F owner=<owner> -F repo=<repo> -F number=<number> \
    -f query='query($owner: String!, $repo: String!, $number: Int!) { repository(owner: $owner, name: $repo) { pullRequest(number: $number) { timelineItems(last: 1, itemTypes: [REMOVED_FROM_MERGE_QUEUE_EVENT]) { nodes { ... on RemovedFromMergeQueueEvent { reason createdAt } } } } } }' \
    --jq '.data.repository.pullRequest.timelineItems.nodes[0] | .reason + " " + .createdAt'
  ```

  If the reason is about checks failing or a conflict, go to "Fixing". If someone removed it by hand, or the reason is anything else, leave the worktree alone and reply `queue-and-clean: fail: PR left the queue: <reason>`.

## Fixing

When the branch is why the PR can't merge (a required check failed, it has merge conflicts, or the merge queue dropped it for one of those), hand it to `/ci-loop`: read the file `switchboard skill-path ci-loop` prints and follow it for this PR, and whichever it is, follow the section "When queue-and-clean runs this" in the text `switchboard skill-path --own ci-loop` prints, Switchboard's own copy, which a repository's copy (or a skill of your own by that name) leaves out. It finds the failure, fixes it in this worktree or reruns a failure the branch didn't cause, pushes without force, and watches until the PR's checks pass. Tell it what brought the PR there: the failing checks, the conflicts, or the merge queue's reason for dropping it.

When it hands back:

- **Green, or a queue failure the branch didn't cause:** read the new head with `gh pr view <number> --json headRefOid` and go back to "Queuing" with it. A push takes the PR out of the merge queue, and `--match-head-commit` has to name the new commit. Keep what it fixed or reran, for the final message.
- **It stopped to ask, and the user answered:** carry on as the answer says.
- **It failed, or stopped for something it leaves alone:** reply `queue-and-clean: fail: <its reason>`.

Hand the PR over at most three times in one run, since each requeue can fail again on a newer base branch. After the third, reply `queue-and-clean: fail: <what still fails>, after <what was tried>`.

## Deploy and test

After the PR merges, and before anything is removed, follow the file `switchboard skill-path deploy-test` prints from "Waiting for the deploy", with what was settled before watching (it is `disable-model-invocation`, so read it rather than invoking it). The worktree stays until the user has approved, so a fix can start from it if the feature isn't right.

- `deploy-test: approved (…)`: bring the main clone up to date and apply the change, as "Applying the change" says, then go to "Cleanup", which closes out the ticket and the note ("Closing out") before the worktree goes, and keep what is in the brackets for the final message.
- `deploy-test: fail: <reason>`: leave everything as it is, the ticket and the note included, and reply `queue-and-clean: fail: <reason>`.

## Cleanup

Only after GitHub reports the PR `MERGED` and the user has approved in "Approval", or, with no PR, once "No PR" has found the default branch holding the work. Follow the file `switchboard skill-path clean-worktree` prints from step 2, with the paths recorded before watching and the PR's number (with no PR, none, and step 2's check of the commits on GitHub counted as done by "No PR") (it is `disable-model-invocation`, so read it rather than invoking it). In a tab marked hands-free (`switchboard show @autopilot_handsfree` prints `1`), starting the item there was the user's ask to remove its worktree and branch once its work has landed, so the removal is never asked about when step 2 finds nothing would be lost; what would be lost is still asked about, the question headed `[always ask]`. Once its step 2 has found nothing would be lost, and before its step 3 removes the worktree, close out the ticket and the note, as "Closing out" says; when step 2 stops, close out nothing.

- `clean-worktree: success`: move the tab to the main clone, as "Back to the repository" says. From here on every command needs `git -C <main-clone>` or an absolute path, since the shell's working directory no longer exists.
- `clean-worktree: fail: <reason>`: reply `queue-and-clean: fail: <reason>`, with what "Closing out" did after it when it ran, since the work is on the default branch whatever the cleanup left.

The only pushes are `/ci-loop`'s fix commits (see "Fixing"), to the PR's own branch, a branch's first push when the user chooses **Open a PR** in "No PR", and the landed commits of "Land on the default branch", once the user says to push them, none of them forced; and, in a repository whose `testIn` is `ci`, the landing's commit to its own `ci/land-<id>`, forced and deleted after, as "The tests in CI" says.

## Applying the change

What the landed change needs to take effect on this computer (Switchboard built and put in place of the one running, the Slack relay restarted, packages installed, migrations run), offered once it is checked out in the main clone, so nothing goes on running the code from before without the user knowing. Skip this section when `switchboard setting apply_after_landing` prints `off` ("Offer to apply a landed change" in Settings).

1. **The range.** After "Land on the default branch", `$before..HEAD` from its step 12. After a PR's merge, bring the main clone up to date first: when it has the PR's base branch checked out, `before=$(git -C <main-clone> rev-parse HEAD)`, `git -C <main-clone> fetch origin <base>` and `git -C <main-clone> merge --ff-only origin/<base>`. When it has another branch checked out, or can't fast-forward, leave it, skip the rest and say so in the final line (`main clone not updated: <why>`), since none of the change runs from there yet. When `before` is still `HEAD`, skip the rest.
2. **What it needs**: `switchboard apply-actions <main-clone> "$before" HEAD`, one action per line, `<key>\t<label>\t<why>\t<command>`, each listed once however many files call for it; an empty command is something only the user can do. When it prints nothing, skip the rest and say nothing.
3. **Hands-free leaves it to the user.** When `switchboard show @autopilot_handsfree` prints `1`, run none of the actions and ask nothing, so the tab never waits on the question with its landing done: keep every action as not applied for step 6, and when step 11 of "Land on the default branch" ran `<lock> autolanded`, run that same command again, its `--plan` or `--decided` included, with `--not-applied '<the labels, joined with ", ">'` before its item (`--not-applied 'Build Switchboard and put it in place of the one running'`), which updates that landing's notification to say so. Go to step 6.
4. **Ask** with `AskUserQuestion`, `multiSelect`, one option per action with a command: its label as the option and its why as the description ("Build Switchboard and put it in place of the one running", "src/sidebar/model.rs changed"). Name the actions with no command in the question's text, each with its why ("Restart the workspace yourself once the new build is in place…: src/sidebar/model.rs changed"); when every action has none, say them in a line instead of asking. A question takes two to four options: a single action is asked with **Not now** as the second option, and more than four are split over more than one question of the same call with at least two in each (five as three and two).
5. **Run the ones chosen**, one at a time, in the order listed, each command as it is, from the main clone. A failure doesn't stop the rest: keep its first line. Nothing here pushes.
6. **Keep what happened** for the final line, `applied: <what ran, a few words each>`, a failure as `<label> failed: <first line>`, and the rest as `not applied: <labels>`; and for the note's `Done:` line, which ends `Applied: <what ran>.` or `Takes effect once <what wasn't done, as the user would do it>.`

## Closing out

Inside the cleanup, once clean-worktree's step 2 has found nothing would be lost and before its step 3 removes the worktree, with the ticket and note settled before watching. It comes first so the item is ticked while its worktree is still there: autopilot and `wtn` count an unticked item whose worktree is gone as free to take again.

1. **The ticket.** Read it with the Linear connector. In a completed state (its state's type is `completed`), there is nothing to do. Otherwise move it to the team's completed state, the one its other merged tickets use ("Done", "Deployed"…), unless the team's workflow moves it there on its own once the PR merges and it hasn't caught up yet: then leave it. Say which, in the final line's note (below).
2. **The note.** Read only the note settled before watching. Find the checklist item for this work: one naming the ticket's ID or link, or the PR's number or link, else one whose words plainly match the ticket's or PR's title; for a branch `/wtn` opened (`note/<slug>`), the item whose first line its slug was made from, as its prompt quoted it. Tick it (`- [x]`) and move it under the unticked items in its list, with anything indented under it, as the user's CLAUDE.md says for notes, changing those lines and nothing else. When no item matches, or more than one might, tick nothing and say so.

A `Done:` line under the ticked item, after a PR's merge or a landing on the default branch alike, says how long the item took right after its hash (`Done: <hash>, 1h12m. …`): the `took` that `switchboard landing --json <worktree>` prints, run here while the worktree is still there; with `took` null (a branch `wtn` didn't start) the line leaves it out. It ends with what "Applying the change" kept for it. Neither step fails the run: the work is on the default branch. Add what they did to the success line, `queue-and-clean: success (ticket ABC-123 done; note: ticked "…")`, or what they couldn't (`note: no item for ABC-123`).

## Back to the repository

Once the worktree is gone, this session runs in a directory that no longer exists, and so does the tab: the workspace, the tab's name and color, and any split or new tab all follow the pane's directory, which a `cd` from inside this session can't change. So after a cleanup that succeeded, restart the pane in the main clone with this same conversation resumed there, as `/wt` restarts its pane (`claude --resume <id>` finds a conversation by its ID from any directory, the deleted one's included).

Only when `$SWITCHBOARD_PANE` and `$CLAUDE_CODE_SESSION_ID` are set and this session was running in the worktree just removed. Otherwise, reply with the final line as usual and stop here.

1. If the main clone's path contains a single quote, reply with the final line and stop: it goes into the command below between single quotes.
2. Write the final line, `queue-and-clean: success (…)` as above, as your reply's text, and in the same reply, as its final action, on its own and not chained to anything:

   ```
   switchboard respawn -c '<main-clone>' claude --resume "$CLAUDE_CODE_SESSION_ID"
   ```

   `respawn` stops this session, and the pane starts again in the main clone with this conversation, the final line included, open where it left off, in a shell of the user's that reads their rc for the PATH. When that Claude exits, a shell is left there. Run nothing after it.
