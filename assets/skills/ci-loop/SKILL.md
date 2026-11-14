---
name: ci-loop
description: Watch the current branch's PR checks in the background and fix what fails, pushing each fix and watching again, until every check passes. Stops and asks when a fix isn't the branch's to make.
argument-hint: "[PR number]"
disable-model-invocation: true
allowed-tools:
  - Bash(switchboard skill-path *)
  - Bash(git fetch)
  - Bash(git fetch *)
  - Bash(gh pr view*)
  - Bash(gh pr checks *)
  - Bash(gh run view *)
  - Bash(gh run list*)
---

# CI loop

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path ci-loop`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Watch this branch's PR checks without blocking the conversation. When a check fails because of this branch, fix it, push, and watch the new run. When it fails for a reason the branch didn't cause, rerun it once. Stop when every check on the PR's latest commit has passed, or when something needs the user.

When the work is done, the final message is one line and nothing else:

- `ci-loop: green` when every check passed, with what you fixed on the way after it: `ci-loop: green (fixed: lint error in api.ts; reran a flaky e2e job)`.
- `ci-loop: fail: <what still fails>, after <what you tried>` when the checks still fail after the last round.
- `ci-loop: stopped: <why>` when the PR merged or closed, or someone else pushed and you were asked to leave it.

Questions asked along the way ("Stop and ask" below) pause the loop rather than end it.

Arguments: a PR number, if one was given. Otherwise use the current branch's PR. `/queue-and-clean` also runs these steps (see "When queue-and-clean runs this").

## Before watching

1. Read the PR:

   ```
   gh pr view <number-if-given> --json number,url,state,isDraft,headRefName,headRefOid,baseRefName
   ```

2. Stop and ask when the PR's `headRefName` is not the branch checked out here, since fixes would land on the wrong branch. If the PR is `MERGED` or `CLOSED`, reply `ci-loop: stopped: PR is <state>`. A draft is fine.
3. If there is no PR for this branch, say so and stop.

## Watching

Start this with the Bash tool's `run_in_background`, with the number and the head commit filled in. You are re-invoked when it exits, so do not poll it yourself and do not sleep in the foreground. Tell the user that you are watching and that they can keep working.

```
sha=<headRefOid>
settled=0
while :; do
  info=$(gh pr view <number> --json state,headRefOid,mergeStateStatus --jq '[.state, .headRefOid, .mergeStateStatus] | @tsv') || { sleep 30; continue; }
  IFS=$'\t' read -r state head merge <<<"$info"
  case "$state" in MERGED) echo merged; exit 0 ;; CLOSED) echo closed; exit 0 ;; esac
  [ "$head" != "$sha" ] && { echo "moved $head"; exit 0; }
  [ "$merge" = DIRTY ] && { echo conflicts; exit 0; }
  # Each check's bucket: pass, fail, pending, skipping or cancel. gh exits
  # non-zero while checks fail or are pending, so only no output is an error.
  buckets=$(gh pr checks <number> --json bucket --jq '[.[].bucket] | unique | join(" ")' 2>/dev/null)
  [ -z "$buckets" ] && { settled=0; sleep 30; continue; }
  case " $buckets " in *" fail "*|*" cancel "*) echo failed; exit 0 ;; esac
  case " $buckets " in
    *" pending "*) settled=0 ;;
    *) settled=$((settled + 1)); [ "$settled" -ge 3 ] && { echo green; exit 0; } ;;
  esac
  sleep 60
done
```

A failed request is retried rather than read as an answer. Green has to hold for three checks in a row, about two minutes, because some checks are only created after others finish, and a commit with no checks reported yet would otherwise look like one with nothing left to run.

When it exits:

- `green`: reply `ci-loop: green`, with what you fixed or reran.
- `failed`: go to "Fixing".
- `conflicts`: go to "Fixing".
- `moved <sha>`: someone pushed to the branch, perhaps the user. Pull it (`git pull --ff-only`) and watch the new commit. If the pull isn't a fast-forward, stop and ask: your work and theirs have diverged.
- `merged` or `closed`: reply `ci-loop: stopped: PR <merged|closed>`.

## Fixing

Do at most five rounds of fix and push in one run. If the checks still fail after the fifth, reply `ci-loop: fail: …`.

**Failing checks.** List them, then read the failing jobs' logs. A check's `link` holds its run ID (`…/actions/runs/<run-id>/job/<job-id>`):

```
gh pr checks <number> --json name,bucket,link --jq '.[] | select(.bucket == "fail" or .bucket == "cancel")'
gh run view <run-id> --log-failed
```

Then decide what kind of failure it is:

- **Caused by this branch:** a test, type check, lint or build step that fails on code this branch changed, or on code that depends on it. Fix it, run the same check locally where the repo lets you, and commit with a message that says what the fix is. Fix only what fails; don't refactor or extend the branch on the way.
- **Not caused by this branch:** a test that fails on code the branch didn't touch, a timeout, a runner or network error, a cancelled job, or a failure that also shows on the base branch. Rerun the failed jobs once, `gh run rerun <run-id> --failed`, and go back to watching the same commit. If the same job fails again, treat it as the branch's after all, or stop and ask if you can't see how the branch causes it.
- **Can't tell from the logs:** stop and ask, with the failing job's name and the relevant lines of its log.

**Merge conflicts, or a branch that has to be up to date.** Merge the base branch into this one rather than rebasing, so the push needs no force: `git fetch origin`, then `git merge origin/<baseRefName>`. Resolve the conflicts so both sides' intent survives, run the tests that cover the conflicted files, and commit the merge. If a conflict can't be resolved without choosing between two behaviors, stop and ask. Rebase only if the user asks for it, since that needs a force-push.

**After each fix:** push with a plain `git push`, never `--force` or `--force-with-lease`. Read the new head with `gh pr view <number> --json headRefOid`, and start "Watching" again with it. Keep track of what each round fixed or reran, for the final message.

## Stop and ask

Instead of fixing, ask the user when:

- the failure is a required review or approval, or anything else that isn't a check;
- the fix would change what the branch does rather than repair it;
- the fix would mean editing CI configuration, skipping, deleting or loosening a test, or lowering a threshold to make a check pass;
- the logs don't show why it failed, or you can't see how the branch causes a failure that keeps coming back.

## When queue-and-clean runs this

`/queue-and-clean` hands its PR here when a check fails, the PR has conflicts, or the merge queue drops it, and takes it back to queue it again when this ends green. Two things differ:

- When the merge queue dropped the PR, the failure is usually in the queue's own run, which tests the branch merged with the latest base branch, not in the PR's checks, which may all have passed. Start from that run. Its branch is named `gh-readonly-queue/<base>/pr-<number>-<sha>`:

  ```
  gh run list --event merge_group --limit 20 --json databaseId,headBranch,conclusion,name \
    --jq '.[] | select(.headBranch | contains("/pr-<number>-")) | select(.conclusion == "failure")'
  ```

  Fix what it shows as above. If it shows a failure the branch didn't cause, there is nothing to rerun on the PR: tell queue-and-clean so, and it queues the PR again unchanged.
- Don't send your own final line. Hand back what you fixed or reran, or why you stopped, and queue-and-clean ends with its own.

Never push anywhere but this branch, never force-push, and never merge the PR: queuing and merging belong to `/queue-and-clean` or the user.
