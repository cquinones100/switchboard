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

Make one page of the tab's change: the story `M-d` tells of it, page by page, with every hunk under the page it belongs to, and publish it with Claude Code's Artifact tool as a private artifact. The story is the diff's own, never one told here, and nothing leaves the computer before the user says yes to what the check in step 4 finds.

## 1. Read the story

Run `switchboard diff-story` in the tab (no directory: it reads the pane's, as `M-d` does). It prints the story as Markdown, from what the diff kept. When it prints "No story told for this change yet" or that `M-d` tells no story, say so in a line, tell the user to open `M-d` (and, for the second, turn the settings it names on) and come back, and stop: never tell a story of your own, and never write a page from the diff alone.

The Markdown is the diff's layout as it is now: the story under `#`, each chapter under `##` (with its map as a Mermaid block when the story is told as a map), each page under `###` with its number, title and why (its claim), a `> ⚠ Watch:` line, its "Builds on" line, and its files, each change's note under its file as `path:line`. Whatever sections it prints (an overview's Before / After and How it gets there, a page's Decided / Instead / Watch, What runs), the page shows them all, in its order and under its names: follow what it prints rather than a fixed list here.

## 2. Read the hunks

The hunks are the branch's commits against the base the diff takes: its PR's base when it has one (`gh pr view --json baseRefName,baseRefOid`), else the default branch (`git rev-parse --abbrev-ref origin/HEAD`), compared from their merge base. Read them with `git diff --no-ext-diff --no-textconv <merge-base>...HEAD`, and, when the story ends in a "Not committed yet" page, the uncommitted changes too (`git diff --no-ext-diff --no-textconv HEAD`). Leave out files git marks binary; name them under their page instead.

Put each file's hunks under the page that lists the file. A change the story's notes give to another page (`path:line` under that page) goes there, with its note over it. A file no page lists goes in a last "Not in the story" section, so no hunk is left out.

## 3. Build the page

Load the `artifact-design` skill first, as the Artifact tool asks. Write one HTML file in the session's scratchpad (`<scratchpad>/diff-artifact-<branch>.html`, the branch from `git branch --show-current`), never in the repository:

- A `<title>` of two to four words naming the change, and the story's title as its heading.
- Each chapter and page in the story's order, with every section the story printed for it (step 1), the Watch line set apart.
- Every hunk in the page itself, as code: its `@@` header, the lines with their old and new numbers, added lines on a green ground with `+`, removed on a red one with `-`, context plain, in a monospace font, wide lines scrolling inside their block rather than the page. Escape every line's `<`, `>` and `&`. Never a link to GitHub or the PR in place of a hunk.
- Colors as tokens on `:root`, redefined for dark mode, so it reads in light and dark.
- No script, font or style from outside but what the Artifact tool allows.

## 4. Check it before it leaves the computer

Read the whole page before publishing it, for:

1. **The plan note's words.** Run `switchboard show @tab_plan_note`, and `switchboard show @tab_note` for the tab's note. When either names a note, read it (in `~/Notes/`, or `$NOTES_DIR`) and find the page's sentences that carry its words (the story may have been told from the plan, with Settings' `diff_story_plan` on). A note is the user's: quote each such sentence.
2. **Secrets and tokens in the hunks**: keys, tokens, passwords, connection strings, private keys, `.env` values.
3. **What `switchboard leak-check` would flag**: run `switchboard leak-check <merge-base>..HEAD` from step 2, and read the page itself for the same kinds the check finds (the words in `~/.config/switchboard/leak-check.txt`, ticket IDs, PR numbers, home paths like `/Users/<name>`, email addresses, token shapes).

Show the user what was found, numbered, one line each with where it is in the page, or say that nothing was, and wait for their word. Publish only after their yes; take out or change what they ask first. A question unanswered is a no.

## 5. Publish

Publish the file with the Artifact tool, with a one-sentence `description` of the change, and give the user its link. It is private, as every artifact starts: never share it further, or with anyone, unless the user asks. Asked to change it later, edit the same file and publish it again to the same link.
