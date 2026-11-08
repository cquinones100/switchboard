---
name: diff-screenshots
description: Render a branch's or PR's diff with git delta, side by side and in colour, as PNG screenshots, one set per file, ordered core code first and tests last with a numbered label for each, then open them in Preview. Use whenever the user wants to see, share, review, present or attach a diff, a branch's changes or a PR as images or screenshots, even if they don't say "delta" or "screenshot" (for example "show me the diff as pictures", "make images of this PR for the deck", "I want to look over the changes in Preview").
argument-hint: "[base ref or PR number] [unified]"
---

# Diff screenshots

This is Switchboard's own copy, for repositories that have none. Before anything else, run `switchboard skill-path diff-screenshots`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Turn a diff into readable images: each changed file rendered by delta with the user's own git config (side-by-side, line numbers), converted to HTML, and screenshotted by Playwright in the system Chrome. The scripts in `scripts/` do the mechanical parts: Node programs using Playwright, kept with this skill rather than made commands of `switchboard`'s, with their packages in `scripts/package.json`; your job is choosing the diff, writing a good label for each file, and checking the result.

## 1. Choose the diff and plan it

The scripts need their packages, which aren't kept with the skill. If `~/.local/share/switchboard/plugin/skills/diff-screenshots/scripts/node_modules` is missing, install them first, at the versions the lockfile pins: `npm ci --prefix ~/.local/share/switchboard/plugin/skills/diff-screenshots/scripts`.


Arguments, all optional: a base ref (`origin/main`, a SHA, `v1.2`), or a PR number (`123` or `#123`), and `unified` if the user asked for a unified diff instead of side-by-side.

Run, from the repository being diffed, with the output going to the session's scratchpad directory (never `/tmp` or the repo; that keeps screenshots out of the work tree and out of other sessions' way):

```bash
OUT=<scratchpad>/diff-screenshots-<short name>
mkdir -p "$OUT"
node ~/.local/share/switchboard/plugin/skills/diff-screenshots/scripts/plan.js [--base <ref> | --pr <number>] > "$OUT/plan.json"
```

With neither flag, the diff is the current branch against its merge base with `main` (`main...HEAD`, or `master` where there is no main). For a PR, the script fetches its head and base by SHA with `gh`, without checking anything out or adding refs. An empty `files` list means there is nothing to show: say so and stop.

The plan orders the files core first, then config, infra and wiring, then docs, then tests (`__tests__/`, `*.spec.*`, `*.test.*`, `e2e/`), keeping git's order within each group. Lockfiles and generated files are listed under `skipped`, since their diffs are thousands of lines nobody reads as a picture. If the grouping is plainly wrong for a file (a source file under a `docs/` folder, say), move it in the list and renumber.

## 2. Label each file

Fill in every file's `label` in `plan.json`: its number, its group, and what changed, inferred from its path and the `hunks` and `sample` lines the plan includes, read more of the diff (`git diff <range> -- <path>`) when those aren't enough. For example:

- `1. Core: retry failed uploads`
- `3. Config: register the new queue worker`
- `4. Docs: README for the export`
- `5. Integration tests` or `6. Tests: retry failed uploads`

A label says what the change does, not just which file it is in; the path is shown under it anyway. Keep it to a few words.

## 3. Render and screenshot

```bash
node ~/.local/share/switchboard/plugin/skills/diff-screenshots/scripts/render.js "$OUT/plan.json" --out "$OUT" [--unified] > "$OUT/shots.json"
```

For each file, it runs `git diff <range> -- <file> | delta --paging=never --width <w>`, so delta reads the user's gitconfig, which already turns on side-by-side, line numbers and navigate. Then it converts delta's ANSI colour to an HTML page (dark background, the label as a heading, a `<pre>` in 13px Menlo at `max-content` width, so nothing reflows), and screenshots it at a 1900px viewport and 2x scale, one PNG per file however long, so a file scrolls as one image.

- Output: `<n>.png` for each file, plus the HTML in `html/`. `shots.json` lists them, each with its size and scale.
- Tall files: Chrome captures at most 16,384 pixels in one go, and the end of a taller page goes missing, so a taller page is captured in slices and stitched into one image. A file over about 465 lines of diff is taken at 1x instead of 2x, which keeps it light enough to scroll. The relay posts images only up to 20 MB, so mention any PNG over that: it won't reach Slack.
- Width: delta wraps any line longer than half its width in side-by-side. The width is 230, or wider for a code file whose longest line needs it (up to 340, past which wrapping is better than a huge image). Docs stay at 230, since prose wraps fine.
- Unified: delta has no `--side-by-side=false`, so the only way to get a unified diff is `--no-gitconfig`, which `--unified` passes. Use it only when the user asks for unified.
- Colour: with `--paging=never`, delta colours its output even into a pipe, so no colour flag is needed.

Playwright comes from the diffed repo's own `@playwright/test` when it has one, otherwise from `scripts/node_modules`. The browser is the system Chrome (`channel: "chrome"`), because Playwright's own browsers often aren't downloaded, and the default launch fails with "Executable doesn't exist"; on Linux that means Google Chrome itself installed (Chromium is not the `chrome` channel), so when the launch fails there, say so rather than downloading a browser. Both `playwright` (1.63.0) and `ansi-to-html` (0.7.2) are pinned in `scripts/package.json`, so the `npm ci` above installs them. Never use npx.

## 4. Check the images

Before reporting, look at the images with Read: at least the first one, and a test file if there is one. Read can't show an image much taller than a screen, so for a tall one, cut out its top and its end and look at those: `node ~/.local/share/switchboard/plugin/skills/diff-screenshots/scripts/crop.js <png> --out "$OUT/check-<n>.png" [--top -1800]`. Check that:

- the colours rendered (green and red line backgrounds, syntax colours), not plain grey text or escape codes;
- lines don't wrap, apart from ones longer than the widest width, or prose in docs;
- a tall image reaches its file's last line, with nothing missing or repeated where its slices join;
- the order and labels are right: core first, tests last, the numbers in sequence.

If something is off, fix it and render again, into a fresh folder, so old images don't mix with new ones.

## 5. Show them

```bash
for png in "$OUT"/*.png; do switchboard platform open "$png"; done
```

opens them in the desktop's image viewer (Preview on macOS). In the final message, list every PNG with its full absolute path, in order, each with its label. Mention any files the plan skipped.
