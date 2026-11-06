---
name: preview-test
description: Wait until an open PR's preview deploy is up, test the feature it adds on the preview in a real browser, record a captioned video of the test, and ask you to approve it. The before-merge counterpart of /deploy-test, using its recorder; run it on any open PR whose repository deploys previews.
argument-hint: "[PR number]"
disable-model-invocation: true
---

# Preview test

This is Switchboard's personal copy, installed for repositories that have none. Before anything else, run `skill-path preview-test`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Test an open PR on its preview deploy before it merges, the way `/deploy-test` tests a merged one on the site it deploys to. This skill only says what differs: read the file `skill-path deploy-test` prints and follow its "Testing in a browser" and "Approval" sections as written (which record with `/walkthrough`'s recorder, `~/.claude/skills/walkthrough/scripts/record.js`), with the changes below, waiting with its `~/.claude/skills/deploy-test/scripts/wait-deploy.js`. Nothing here changes the repository or the PR.

When it is done, the final message is `preview-test: approved (tested: <video>)`, `preview-test: approved (not tested: <why>)` or `preview-test: fail: <reason>`, and nothing else.

Arguments: a PR number. Otherwise use the current branch's PR. If it is `MERGED` or `CLOSED`, say so and stop: a merged one is `/deploy-test`'s, and a closed one's preview is likely gone.

## Settling

Read the PR and the repo: `gh pr view <number> --json number,url,state,title,body,headRefName,headRefOid` and `gh repo view --json owner,name`. The branch name is the PR author's to choose, and may hold quotes, `;` or `$(…)`, so never paste it into a command: each command below reads it into a shell variable from gh, as written, and quotes it (`"$ref"`), and `gh api` takes it as a field (`-f ref=…`, with `--method GET`) rather than in the URL. Then settle:

1. **The preview**: where the PR's head commit is deployed for review, and what shows it is up. Look, in this order:
   - the GitHub deployments of the PR's branch, or of its head commit, as some actions record them, which Vercel, Netlify, Render and most preview actions report:

     ```
     ref=$(gh pr view <number> --json headRefName --jq .headRefName)
     gh api --method GET 'repos/<owner>/<repo>/deployments' -f ref="$ref" -f per_page=20 --jq '[.[].environment] | unique'
     gh api --method GET 'repos/<owner>/<repo>/deployments' -f ref='<headRefOid>' -f per_page=20 --jq '[.[].environment] | unique'
     gh api --method GET 'repos/<owner>/<repo>/deployments' -f ref="$ref" -f environment='<env>' -f per_page=1 --jq '.[0].id' |
       xargs -I{} gh api 'repos/<owner>/<repo>/deployments/{}/statuses?per_page=1' --jq '.[0].environment_url'
     ```

   - a preview link a GitHub App commented on the PR, the newest one: `gh api 'repos/<owner>/<repo>/issues/<number>/comments?per_page=100' --jq '.[] | select(.user.type == "Bot") | .body'`. Anyone can comment on a public repository's PR, and a bot's account is one only an installed app has; even so, say which link you took and from whom, and ask before opening it when its host isn't the deploy provider's (`vercel.app`, `netlify.app`, `onrender.com` and the like) or the repository's own;
   - the repository's `CLAUDE.md`, `AGENTS.md` and README, for how previews are made and named.

   When one answer is plain, use it and say in a line which; otherwise ask, with **No preview to wait for** (give the URL to test, or test nothing) and **Skip the test** as options.
2. **The test**: as `/deploy-test`'s "Settling" says, from the PR's description, its Linear ticket's acceptance criteria and its diff (`gh pr diff <number>`).

## Waiting for the preview

Skip this when the preview has no GitHub deployment to watch (a URL from a comment, or one the user gave). A comment's link is tested as it is only when the comment names the PR's head commit (`headRefOid`, or its first seven characters) and says that deploy is done (Ready, Deployed, a check mark), not building or queued; otherwise nothing ties it to that commit and it may be an older build's, so ask whether to test it anyway, wait for a newer comment, or stop. Otherwise watch the head commit with the Bash tool's `run_in_background`, as `/deploy-test` does, so you are re-invoked when it exits:

```
ref=$(gh pr view <number> --json headRefName --jq .headRefName)
node ~/.claude/skills/deploy-test/scripts/wait-deploy.js --repo <owner>/<repo> --sha <headRefOid> --environment '<env>' --ref "$ref" --timeout 20
```

A deploy of the head commit counts, and so does one of a commit pushed after it, when that deploy is recorded under the branch (one recorded under its own commit is found by running this again for the new head); `--ref` keeps it to this PR's branch, since PRs share a preview environment and one branched from this PR would count too. On `deployed <sha> <url>`, test that URL, the one this commit's preview has, or, when it is `-` (a deploy that gave none), ask for the URL to test, offering the one settled above with the commit it was deployed from, since it may be an older build's; on `failed <url>`, reply `preview-test: fail: preview deploy failed: <url>`; on `timeout`, ask whether to keep waiting, test the last preview anyway, or stop.

## Testing in a browser

As `/deploy-test`'s section says, with these changes:

- **Files** go in `<scratchpad>/preview-test-<number>/`, the video too, and stay there, as `/deploy-test` says.
- **Change nothing real, as `/deploy-test` says, preview or not**: a preview's own address says where the page runs, not which database or APIs it talks to, and many previews use production's. Submitting, saving or creating test data is fine only when the repository's docs say its previews have data of their own, or the user says so when asked. Never send real email or pay for anything.
- **One profile per repository**: a preview's host changes with every PR, so record with `--profile preview-<owner>--<repo>` (two dashes, which no GitHub owner's name has, so no two repositories share it), and sign in with `node ~/.claude/skills/walkthrough/scripts/record.js --login --url <url> --profile preview-<owner>--<repo>` when the preview needs it (a Vercel or Netlify protection page is a sign-in page). Once signed in there, later PRs' previews of the same repository reuse it.

## Approval

As `/deploy-test`'s section says, replying `preview-test: …` in place of `deploy-test: …`. Saying something on the PR (a comment, a review) is outward-facing: offer it only when the user asks.
