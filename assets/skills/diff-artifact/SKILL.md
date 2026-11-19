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

The Markdown holds what the diff's Story view shows: the story under `#`; `## What changes` with its Before and After; `## What runs`, the steps in the order they happen, each naming its page; each chapter under `## Chapter N: <title>`, each page under `###` with its number in its chapter, title, claim, why, `Decided:` and `Instead`, a `> ⚠ Watch:` line and its files, each change's note under its file as `path:line`; and `## In the files only` for what no chapter holds. Whatever sections it prints, the page shows them all, in its order and under its names: follow what it prints rather than a fixed list here.

## 2. Read the hunks

The hunks are the branch's commits against the base the diff takes: its PR's base when it has one (`gh pr view --json baseRefName,baseRefOid`), else the default branch (`git rev-parse --abbrev-ref origin/HEAD`), compared from their merge base. Read them with `git diff --no-ext-diff --no-textconv <merge-base>...HEAD`, and, when the story ends in a "Not committed yet" page, the uncommitted changes too (`git diff --no-ext-diff --no-textconv HEAD`). Leave out files git marks binary; name them under their page instead.

Put each file's hunks under the page that lists the file. A change the story's notes give to another page (`path:line` under that page) goes there, with its note over it. A file no page lists goes in a last "Not in the story" section, so no hunk is left out.

## 3. Build the page

Load the `artifact-design` skill first, as the Artifact tool asks. Write one HTML file in the session's scratchpad (`<scratchpad>/diff-artifact-<branch>.html`, the branch from `git branch --show-current`), never in the repository:

The page follows the diff's Story view (boards 140 to 142, 144 and 145), so reading it is reading `M-d`:

- A `<title>` of two to four words naming the change.
- **What runs** across the top, as the view lays it over the column: a box a step in the order it happens, an arrow between, each naming its page and linking to it, the row scrolling sideways when it is wider than the page.
- **The outline** at the left (over the column on a narrow screen): each chapter with What changes first under chapter 1 and its pages indented under it, each a link to its section, then In the files only.
- **The overview** at the head of the column: the story's title, its size (chapters, pages, files, lines added and removed) and what it is compared with; Before and After side by side with an arrow between (one over the other on a narrow screen); How it gets there, the first chapter's pages each with the first sentence of its why; the other chapters named; and what it was told from and by which model, when the story says.
- **The column**: each page a section in the story's order, headed with its number, title and place ("page 1 of 4 · chapter 1"), its claim, a block of Decided, Instead and Watch set apart on a surface of its own, then each file it changes with its hunks under it, the prose kept to about 88 characters wide, and a file of tests last, folded in a `<details>`.
- Every hunk in the page itself, as code: its `@@` header, then its lines side by side where the page is wide enough, the old at the left and the new beside it, the words that changed in a stronger shade, and one over the other on a narrow screen (a CSS media query, no script), removed marked `-` on a red ground over added marked `+` on a green one; context plain, line numbers on each side, in a monospace font, wide lines scrolling inside their block rather than the page. Escape every line's `<`, `>` and `&`. Never a link to GitHub or the PR in place of a hunk.
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
