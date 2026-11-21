---
name: diff-artifact
description: Turn the tab's diff and the story M-d tells of it into one HTML page, every hunk in it as colored code, and publish it as a private Claude artifact, after checking it for what shouldn't leave the computer. Use when the user wants the diff's story as a page, a link, an artifact or something to open in a browser or share, even if they don't say "artifact" (for example "make a page of this change", "give me a link to the story").
allowed-tools:
  - Bash(switchboard skill-path *)
  - Bash(switchboard diff-story)
  - Bash(switchboard diff-story *)
  - Bash(switchboard show *)
  - Bash(git rev-parse *)
  - Bash(git branch --show-current)
  - Bash(gh pr view*)
---

# Diff artifact

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path diff-artifact`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Make one page of the tab's change: the story `M-d` tells of it, page by page, with every hunk under the page it belongs to, and publish it with Claude Code's Artifact tool as a private artifact. The story is the diff's own, never one told here, and nothing leaves the computer before the user says yes to what the check in step 3 finds.

## 1. Read the story

Run `switchboard diff-story` in the tab (no directory: it reads the pane's, as `M-d` does). It prints the story as Markdown, from what the diff kept, which step 3 reads for what shouldn't leave the computer. When it prints "No story told for this change yet" or that `M-d` tells no story, say so in a line, tell the user to open `M-d` (and, for the second, turn the settings it names on) and come back, and stop: never tell a story of your own, and never write a page from the diff alone.

## 2. Draw the page

The page is the diff's Story view as `switchboard-diff` draws it alone, drawn by the diff's own code, never written by hand: its header line, What runs over the column, the outline at the left, the overview at the head of the column, then each page with its claim and how, its Decided, Instead and Watch block and every hunk inline as the view shows it (side by side, the words that changed in the stronger shade), files of tests unfolded, in the dark theme and the light, the page showing the one the reader's browser asks for. Never a link to GitHub or the PR in place of a hunk: every hunk is in the page.

Run `switchboard diff-story --html > <scratchpad>/diff-artifact-<branch>.html` in the tab (the session's scratchpad, never the repository; the branch from `git branch --show-current`). `--cols <n>` draws it `<n>` columns wide (170 unless said, wide enough for the hunks side by side). When it prints nothing, the story is told as a list, which has no Story view: say so and that Settings' "Open the diff on what changes" (`diff_story_map`) tells it in chapters, and stop. What the Story view leaves to the files (In the files only) isn't in the page; say so when the outline lists any.

## 3. Check it before it leaves the computer

Read everything the page holds before publishing it: its words are the story's Markdown from step 1 and the branch's hunks (`git diff --no-ext-diff --no-textconv <merge-base>...HEAD`, and `git diff --no-ext-diff --no-textconv HEAD` when anything isn't committed yet), drawn cell by cell, and its header names the repository's directory from `~`. Read them for:

1. **The plan note's words.** Run `switchboard show @tab_plan_note`, and `switchboard show @tab_note` for the tab's note. When either names a note, read it (in `~/Notes/`, or `$NOTES_DIR`) and find the page's sentences that carry its words (the story may have been told from the plan, with Settings' `diff_story_plan` on). A note is the user's: quote each such sentence.
2. **Secrets and tokens in the hunks**: keys, tokens, passwords, connection strings, private keys, `.env` values.

Show the user what was found, numbered, one line each with where it is in the page, or say that nothing was, and wait for their word. Publish only after their yes; take out or change what they ask first. A question unanswered is a no.

## 4. Publish

Publish the file with the Artifact tool, with a one-sentence `description` of the change, and give the user its link. It is private, as every artifact starts: never share it further, or with anyone, unless the user asks. Asked to change it later, edit the same file and publish it again to the same link.
