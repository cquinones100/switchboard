---
name: walkthrough
description: Record a captioned video of a feature in a real browser, reached the way a person would reach it, with every click, highlight and the page's address shown, and hand back an MP4 to share. Use when asked for a walkthrough, a demo video, a screen recording of a feature, or a video to put in a PR, a ticket or Slack. Its recorder is the one /deploy-test and /preview-test record their tests with.
argument-hint: "[what to show, a PR number, or a URL]"
disable-model-invocation: true
---

# Walkthrough

Record a video of a feature as someone would show it to a teammate: start where they would start, click through to it, and say what each part shows. The recorder is `scripts/record.js` beside this file, which `/deploy-test` and `/preview-test` use too, so a change to how it records reaches all three; those skills read this file's "Recording" and "Reviewing the video" sections.

When it is done, the final message is `walkthrough: <video>` or `walkthrough: fail: <reason>`, and nothing else. The questions below still get asked, because they pause the work rather than end it.

Arguments: what to show, in words; a PR number (show what it changes); or a URL to start from. With none, ask what to show.

## Where to record

Settle the site to record on, and say in a line which, asking when it isn't plain:

- **A local dev server**: the app this repository runs, started as the repository says (its `CLAUDE.md`, `AGENTS.md`, README, or the `run` skill's patterns). Use it when the work isn't deployed yet, or the user says so.
- **A PR's preview**: settle and wait for it as `~/.claude/skills/preview-test/SKILL.md`'s "Settling" and "Waiting for the preview" say.
- **The deployed site**: as `~/.claude/skills/deploy-test/SKILL.md`'s "Settling" says, for something already merged and live.

Record from the site's home page (`--url` its root), never a deep link to the feature: the video shows how to get there.

## Data

A walkthrough shows a feature with realistic data in it. Seed it first only where nothing real can be touched: a local server's own database (through the app's seed scripts or its UI), or a preview whose repository's docs say its data is its own. Anywhere else, `/deploy-test`'s "Change nothing real" holds: record with what is already there, and say so when the result looks thin. Never create, send, pay for or email anything real to make a better video.

## Recording

The scripts need their packages: if `~/.claude/skills/walkthrough/scripts/node_modules` is missing, run `npm ci --prefix ~/.claude/skills/walkthrough/scripts`. Recording needs Playwright's own ffmpeg too; if a run says it is missing, run `node ~/.claude/skills/walkthrough/scripts/node_modules/playwright/cli.js install ffmpeg`. The video (its address bar and captions, the MP4) and the contact sheet need the system's `ffmpeg`; without it the recorder stops before recording and says so (`brew install ffmpeg`).

1. **Write the steps** in the session's scratchpad, `<scratchpad>/walkthrough-<slug>/steps.js`, exporting one async function given `{ page, step, highlight, expect }` (`record.js` has an example):
   - Start each part with `step("<what this shows>")`, which captions it in a strip under the page, kept across navigations.
   - Reach the feature through the app's own navigation, a step naming each click ("Campaigns › Opening Deck Demo › Opening Deck tab").
   - `highlight(locator)` outlines what is being shown, until the next `step`.
   - `expect` what should be on screen, so a broken page shows in the log.
   - Prefer roles and visible text to CSS selectors.
   - Keep it to about a minute.

   The recorder draws the rest itself: a cursor that moves to each thing clicked, with a ripple where it clicks, and, above the page, an address bar as a browser draws one with the page's whole address, kept current through client-side navigation. The address bar and the caption take space of their own above and below the page, so neither covers it.
2. **Record it**:

   ```
   node ~/.claude/skills/walkthrough/scripts/record.js <scratchpad>/walkthrough-<slug>/steps.js \
     --url <site root> --out <scratchpad>/walkthrough-<slug> --pace demo \
     --name "<what it shows>"
   ```

   `--name` names the video: a few words you write saying what the feature does, as someone would look for it in a folder ("Filter orders by status", "Invite a teammate to a campaign"), never the branch or ticket ID. The recorder makes it a file name (`Filter-orders-by-status.mp4`). Without it, the video is named after the tmux tab.

   `--pace demo` holds each step long enough to read and carries on past an `expect` that fails, noting it in the log; `/deploy-test` and `/preview-test` record at the default `--pace test`, which stops at the first failure. A site whose host changes (a preview) takes `--profile`, as `/preview-test` says. It prints `{ passed, video, contact, screenshot, log, error }`.
3. **Signing in.** Each site keeps its own Chrome profile under `~/.cache/deploy-test/profiles/`. If the recording lands on a sign-in page, ask the user to sign in, then run `node ~/.claude/skills/walkthrough/scripts/record.js --login --url <url>` (with the same `--profile`, if any) in the background: it opens a visible window, and they close it once signed in. Then record again. Never type a password or a code yourself.

## Reviewing the video

Before showing anyone the video, look at it:

- **The contact sheet** (`contact`, a frame every two seconds): Read it, and check every caption reads well, each step shows what its caption says, nothing on the page is cut off, the log notes no caption cut short (shorten one that is), and no frame shows an error page, a spinner that never ends or a sign-in page.
- **The log**: the steps, console errors, responses of 500 and up, and failed expects.

Fix what is wrong in `steps.js` and record again, at most three times. A feature that doesn't do what it should is something to tell the user, not to hide by leaving it out of the video.

Then open the video with `open <video>`, so it plays in QuickTime, and ask with `AskUserQuestion`, saying in the question what it shows, in a line, anything the log caught, and the video's full path. The options:

- **Keep it**: copy it to `~/Movies/Walkthroughs/` (made if missing), keeping its name, with `-2`, `-3` and so on before the extension when a video there already has it, then tidy that folder as below, and reply `walkthrough: <its path there>`.
- **Record again**: with what the user says to change (their notes on the option), change the steps and record again.
- **Drop it**: reply `walkthrough: fail: <their reason>`.

### Tidying the kept videos

Each time a video is kept, look at what `~/Movies/Walkthroughs/` holds: each video's name, when it was made and its size (`ls -lhtT`). When any is over 30 days old, ask with `AskUserQuestion`, listing those with their dates and sizes and the folder's total size, whether to move them to the Trash: **Move them all**, **Choose which** (then a second question, `multiSelect`, the old ones as options, four at a time), or **Keep them**. Move a video with Finder, so it can be taken back from the Trash (`osascript -e 'tell application "Finder" to delete POSIX file "<path>"'`), never with `rm`, and only ones in that folder. With none over 30 days old, ask nothing.

Putting the video anywhere outside this Mac (a PR, a ticket, Slack) is outward-facing: offer it only when the user asks.
