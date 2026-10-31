---
name: deploy-test
description: Wait until a merged PR has deployed, test the feature it adds on the deployed site in a real browser, record a captioned video of the test, and ask you to approve it. Queue-and-clean runs it before removing a worktree; run it yourself to test any merged PR.
argument-hint: "[PR number]"
disable-model-invocation: true
---

# Deploy test

Wait for a merged PR to reach the site it deploys to, walk through what it changed in Chrome while recording a video, and ask the user to approve what the video shows. Nothing here changes the repository: it needs only the PR and its repo, so it runs from any clone, or from `/queue-and-clean`, which reads this file and follows it (see "From queue-and-clean").

When it is done, the final message is `deploy-test: approved (tested: <video>)`, `deploy-test: approved (not tested: <why>)` or `deploy-test: fail: <reason>`, and nothing else. The questions in "Settling", "Testing in a browser" and "Approval" are still asked, because they pause the work rather than end it.

Arguments: a PR number. Otherwise use the current branch's PR. If it isn't `MERGED`, say so and stop: there is nothing deployed to test yet.

## Settling

Read the PR and the repo: `gh pr view <number> --json number,url,state,title,body,baseRefName,mergeCommit` and `gh repo view --json owner,name`. Then settle:

1. **The deploy**: what shows the base branch has deployed, and the URL to test it at. Look, in this order: the repository's `CLAUDE.md`, `AGENTS.md` and README for how and where it deploys; the GitHub environments its recent deploys went to, with their URLs,

   ```
   gh api 'repos/<owner>/<repo>/deployments?ref=<baseRefName>&per_page=20' --jq '[.[].environment] | unique'
   gh api 'repos/<owner>/<repo>/deployments?environment=<env>&per_page=1' --jq '.[0].id' |
     xargs -I{} gh api 'repos/<owner>/<repo>/deployments/{}/statuses?per_page=1' --jq '.[0].environment_url'
   ```

   and the workflows that run on pushes to the base branch, for one that deploys (`gh run list --branch <baseRefName> --event push --limit 30 --json workflowName --jq 'map(.workflowName) | unique'`). Prefer the environment the merge reaches first and that people use, usually production or staging, over previews. When one answer is plain, use it and say in a line which; otherwise ask, with **No deploy to wait for** (test what is already live, or nothing) and **Skip the deploy and the test** as options.
2. **The test**: what the PR changes that a person could see in a browser, from its description, the acceptance criteria of the ticket its branch or title names (read with `~/.config/tmux/tracker.sh show <ID>`, which asks Linear or Jira as the tracker setting says, with Switchboard's own key or token: exit 2 means the setting is off and there is no ticket to read; on any other failure, no key stored or a 3 (no such ticket, which Jira also answers for one the token's account can't see), read it through the tracker's connector instead, settling which tracker as `~/switchboard/claude/skills/shape/tracker.md`'s "Which one" says and reading as its "Reading" says, and only when that can't find it either say in a line that there is no such ticket), and the diff (`gh pr diff <number>`). A backend, infrastructure, tooling or library change may have nothing to see; then there is no browser test, and "Approval" says so.

## Waiting for the deploy

Skip this when the deploy settled in "Settling" was **No deploy to wait for** or **Skip the deploy and the test**. Otherwise read the merge commit, `gh pr view <number> --json mergeCommit --jq .mergeCommit.oid`, and start the watch with the Bash tool's `run_in_background`, so the conversation is not blocked; you are re-invoked when it exits, so don't poll it:

```
node ~/.claude/skills/deploy-test/scripts/wait-deploy.js --repo <owner>/<repo> --sha <merge commit> \
  --environment <env>                                   # a GitHub environment, or
  --workflow <workflow> --branch <baseRefName>          # a deploy workflow on the base branch
```

It counts a deploy of the merge commit or of any later commit that includes it, and prints one line:

- `deployed <sha> <url>`: go on to the test. A URL other than `-` is where that deploy went, and wins over the one settled when they differ only by a preview's address.
- `failed <url>`: every deploy that includes the merge failed. Reply `deploy-test: fail: deploy failed: <url>`.
- `timeout`: nothing deployed it in 45 minutes. Ask whether to keep waiting (run it again), test what is live anyway, or stop, replying `deploy-test: fail: no deploy after 45 minutes`.

## Testing in a browser

Skip this when there is nothing a browser can show, or the test was skipped, and go to "Approval". Otherwise the scripts need their packages: if `~/.claude/skills/deploy-test/scripts/node_modules` is missing, run `npm ci --prefix ~/.claude/skills/deploy-test/scripts`. Recording needs Playwright's own ffmpeg too; if a run says it is missing, run `node ~/.claude/skills/deploy-test/scripts/node_modules/playwright/cli.js install ffmpeg`.

1. **Write the test** in the session's scratchpad, `<scratchpad>/deploy-test-<number>/steps.js`, exporting one async function given `{ page, step, expect }` (`record.js` has an example). Walk through the change as a person would: go where the feature is, do what the acceptance criteria describe, and `expect` what should be on screen. Start each part with `step("<what this shows>")`, which captions the video, so the user can follow it without the code. Prefer roles and visible text to CSS selectors. Keep it under a minute.
2. **Change nothing real.** This is the deployed site, often production, with real people's data. Look, navigate, open and fill in what you need to, but don't submit what saves, sends, deletes, pays for or emails anything, unless the environment is plainly a test one (staging, a preview) or the user says so when asked. When the feature can only be seen by changing data, ask first, saying what the test would change. A feature behind a flag that is off is something to report, not to turn on.
3. **Record it:**

   ```
   node ~/.claude/skills/deploy-test/scripts/record.js <scratchpad>/deploy-test-<number>/steps.js \
     --url <url> --out <scratchpad>/deploy-test-<number>
   ```

   It runs the test headless in the system Chrome and prints `{ passed, video, screenshot, log, error }`. Look at the screenshot and the log (each step, console errors, and responses of 500 and up) as well as `passed`.
4. **Signing in.** Each site keeps its own Chrome profile under `~/.cache/deploy-test/profiles/`. If the test lands on a sign-in page, ask the user to sign in, then run `node ~/.claude/skills/deploy-test/scripts/record.js --login --url <url>` in the background: it opens a visible window, and they close it once signed in. Then record again. Never type a password or a code yourself.
5. **When it fails**, decide whether the test or the feature is wrong. Fix a test that was wrong (a selector, a timing, a step in the wrong place) and record again, at most three times. A feature that doesn't do what it should is the answer, not something to fix here: take that video to "Approval".

## Approval

Open the video with `open <video>`, so it plays in QuickTime, and ask with `AskUserQuestion`. Say in the question what the test showed, in a line, whether it passed, anything the log caught, and the video's full path; or, when there was no test, why not. The options:

- **Approve**: reply `deploy-test: approved (tested: <video>)`, or `deploy-test: approved (not tested: <why>)`.
- **Record again**: with what the user says to change (their notes on the option), change the test and go back to recording.
- **Not right**: reply `deploy-test: fail: test not approved: <their reason, or what the test showed>`.

## From queue-and-clean

`/queue-and-clean` settles the deploy and the test as "Settling" says before it watches the PR, so it asks nothing once the PR has merged, and follows the rest after the merge, before it removes anything: the worktree stays until the user has seen the feature work, so a fix can start from it if it doesn't. Its final line takes this skill's result in its brackets (`tested: <video>` or `not tested: <why>`), and turns a `fail` here into its own, leaving the worktree, the ticket and the note as they are.
