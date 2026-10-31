---
name: queue-and-clean
description: Queue the current branch's PR for merge if it is not queued already, watch it in the background, fix what stops it merging (failing checks, conflicts, being dropped from the merge queue), and once it merges wait for it to deploy, test the feature in a real browser and record a video of the test, and once you approve the test make sure its ticket (Linear or Jira) is done and its item in your notes ticked, then remove this worktree and its local branch.
argument-hint: "[PR number]"
disable-model-invocation: true
---

# Queue and clean

Queue this worktree's PR for merge, in the merge queue or with auto-merge, unless it is already queued. Then watch the PR without blocking the conversation. When something the branch can fix stops it merging (a required check fails, it has merge conflicts, or the merge queue drops it), fix it, push, queue it again and go back to watching, as described in "Fixing". When it merges, wait until the merge has deployed, test the feature on the deployed site in a real browser, recording a video of the test, and ask the user to approve it ("Deploy and test"). Once they approve, remove the worktree and its local branch. If it closes without merging, or stops for something you can't fix, leave everything as it is.

When the work is done, the final message is either `queue-and-clean: success` or `queue-and-clean: fail: <reason>` and nothing else: no summary of what was checked or what was removed. `success` means the PR merged, the user approved the test (or chose to clean up without one) and the cleanup finished; if you fixed anything on the way, say what in a few words after it, as `queue-and-clean: success (fixed: <what>)`, for example `queue-and-clean: success (fixed: lint error in api.ts; reran a flaky e2e job)`. What "Deploy and test" and "Closing out" did goes in the same brackets, after anything fixed: `queue-and-clean: success (tested: <video>; ticket ABC-123 done)`. `fail` means the PR could not be queued, closed without merging, failed to deploy, had its test turned down, stopped for something "Fixing" leaves alone or still failed after its last round, or a cleanup command errored, and the reason is one line saying which, with the error message when a command failed (for example `queue-and-clean: fail: git worktree remove: '<path>' contains modified or untracked files`), and what you tried when you tried fixing it. The questions in "Before watching", the merge method question in "Queuing", the questions in "Fixing", the questions and the approval in "Deploy and test", and the questions in clean-worktree's step 2 are still asked, because they pause the work rather than end it.

Arguments: a PR number, if one was given. Otherwise use the current branch's PR.

## Before watching

1. Read the PR and the repo:

   ```
   gh pr view <number-if-given> --json number,url,state,isDraft,headRefName,headRefOid,autoMergeRequest
   gh repo view --json owner,name --jq '.owner.login + " " + .name'
   ```

2. Record the paths the cleanup needs now, while the worktree still exists, as step 1 of `~/switchboard/claude/skills/clean-worktree/SKILL.md` finds them:

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
   - **The ticket**: the ticket the branch names (an ID like `ABC-123` at the start of the branch name), else one the PR's title or body names. None is fine: then there is no ticket to close out.
   - **The note**: which of the user's notes holds this work's checklist item, settled as `~/.claude/skills/note-watch/pick-note.md` says, with **No note** as an option, since not every PR has one. Remember the answer; read the note only at "Closing out".
   - **The deploy and the test**: settle both as "Settling" in `~/switchboard/claude/skills/deploy-test/SKILL.md` says, so nothing after the merge has to ask.

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

When the branch is why the PR can't merge (a required check failed, it has merge conflicts, or the merge queue dropped it for one of those), hand it to `/ci-loop`: read the repository's own `.claude/skills/ci-loop/SKILL.md` and follow it for this PR, or `~/switchboard/claude/skills/ci-loop/SKILL.md` when the repository has none, and in either case follow the section "When queue-and-clean runs this" in `~/switchboard/claude/skills/ci-loop/SKILL.md`, which a repository's copy leaves out. It finds the failure, fixes it in this worktree or reruns a failure the branch didn't cause, pushes without force, and watches until the PR's checks pass. Tell it what brought the PR there: the failing checks, the conflicts, or the merge queue's reason for dropping it.

When it hands back:

- **Green, or a queue failure the branch didn't cause:** read the new head with `gh pr view <number> --json headRefOid` and go back to "Queuing" with it. A push takes the PR out of the merge queue, and `--match-head-commit` has to name the new commit. Keep what it fixed or reran, for the final message.
- **It stopped to ask, and the user answered:** carry on as the answer says.
- **It failed, or stopped for something it leaves alone:** reply `queue-and-clean: fail: <its reason>`.

Hand the PR over at most three times in one run, since each requeue can fail again on a newer base branch. After the third, reply `queue-and-clean: fail: <what still fails>, after <what was tried>`.

## Deploy and test

After the PR merges, and before anything is removed, follow `~/switchboard/claude/skills/deploy-test/SKILL.md` from "Waiting for the deploy", with what was settled before watching (it is `disable-model-invocation`, so read it rather than invoking it). The worktree stays until the user has approved, so a fix can start from it if the feature isn't right.

- `deploy-test: approved (…)`: go to "Cleanup", and keep what is in the brackets for the final message.
- `deploy-test: fail: <reason>`: leave everything as it is, the ticket and the note included, and reply `queue-and-clean: fail: <reason>`.

## Cleanup

Only after GitHub reports the PR `MERGED` and the user has approved in "Approval". Follow `~/switchboard/claude/skills/clean-worktree/SKILL.md` from step 2, with the paths recorded before watching and the PR's number (it is `disable-model-invocation`, so read it rather than invoking it).

- `clean-worktree: success`: close out the ticket and the note, as "Closing out" says, then move the tab to the main clone, as "Back to the repository" says. From here on every command needs `git -C <main-clone>` or an absolute path, since the shell's working directory no longer exists.
- `clean-worktree: fail: <reason>`: reply `queue-and-clean: fail: <reason>`, closing out nothing.

The only pushes are `/ci-loop`'s fix commits (see "Fixing"), to the PR's own branch, and never a force-push.

## Closing out

After the cleanup, with the ticket and note settled before watching:

1. **The ticket.** It is in Linear or Jira, whichever the tracker setting names: settle which, and how to read and move a ticket in it, as `~/switchboard/claude/skills/shape/tracker.md` says (with the setting off, there is no ticket to close out). Read it (`~/.config/tmux/tracker.sh show <ID>` will do; when it fails other than with exit 2 (the setting off), such as no key stored or a 3 (no such ticket, which Jira also answers for one the token's account can't see), through the tracker's connector, and only when that can't find it either is there no ticket to close out, as tracker.md's "Reading" says). In a completed state (its state's type is `completed`, a Jira status in the Done category), there is nothing to do. Otherwise move it to the team's completed state, the one its other merged tickets use ("Done", "Deployed"…; in Jira, the transition tracker.md's "Done" picks), unless the team's workflow moves it there on its own once the PR merges and it hasn't caught up yet: then leave it. Say which, in the final line's note (below).
2. **The note.** Read only the note settled before watching. Find the checklist item for this work: one naming the ticket's ID or link, or the PR's number or link, else one whose words plainly match the ticket's or PR's title. Tick it (`- [x]`) and move it under the unticked items in its list, with anything indented under it, as the user's CLAUDE.md says for notes, changing those lines and nothing else. When no item matches, or more than one might, tick nothing and say so.

Neither step fails the run: the PR merged and the cleanup finished. Add what they did to the success line, `queue-and-clean: success (ticket ABC-123 done; note: ticked "…")`, or what they couldn't (`note: no item for ABC-123`).

## Back to the repository

Once the worktree is gone, this session runs in a directory that no longer exists, and so does the tab: tmux, the tab's name and color, and any split or new tab all follow the pane's directory, which a `cd` from inside this session can't change. So after a cleanup that succeeded, restart the pane in the main clone with this same conversation resumed there, as `/wt` restarts its pane (`claude --resume <id>` finds a conversation by its ID from any directory, the deleted one's included).

Only when `$TMUX_PANE` and `$CLAUDE_CODE_SESSION_ID` are set and this session was running in the worktree just removed. Otherwise, reply with the final line as usual and stop here.

1. If the main clone's path contains a single quote, reply with the final line and stop: it goes into the command below between single quotes.
2. Keep the tab's title. It comes from the worktree's branch (its ticket and PR), so in the main clone, on its default branch, it would change to the repository's name. Pin the title it has now as the tab's name, which wins over the branch:

   ```
   tmux set-option -w -t "$TMUX_PANE" @tab_name "$(tmux show-option -wqv -t "$TMUX_PANE" @tab_label)"
   ```

   Skip this if `@tab_label` is empty.
3. Write the final line, `queue-and-clean: success (…)` as above, as your reply's text, and in the same reply, as its final action, on its own and not chained to anything:

   ```
   tmux respawn-pane -k -t "$TMUX_PANE" -c '<main-clone>' "zsh -i -c 'claude --resume $CLAUDE_CODE_SESSION_ID; exec zsh -i'"
   ```

   `-k` stops this session, and the pane starts again in the main clone with this conversation, the final line included, open where it left off. When that Claude exits, `exec zsh -i` leaves a shell there. Run nothing after it.
