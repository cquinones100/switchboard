---
name: design-docs
description: Publish the UX 2.0 design's boards to this repository as web pages for GitHub Pages - render each board's source in docs/design/src as a static HTML page in docs/design after checking its text holds only made-up names, rewrite docs/design/README.md, and push the change straight to main when it touches docs/design alone. For Switchboard's own repository. Use when asked to sync, publish or update the design docs, after changing a board, or runs /design-docs.
disable-model-invocation: true
---

# Publish the design's boards

Switchboard's own skill for `docs/design/`. Each UX 2.0 board is a source in `docs/design/src/` (its `<Name>.dc.html`, with `canvas.json` listing every board, its title and its page), first copied from the UX 2.0 canvas, a claude.ai Design artifact kept now as a read-only archive. A board is changed by editing its source, and a new one is a new source with its entry in `canvas.json` and its row in `Index.dc.html`'s groups. This repository is public, so what lands is each board's source and its page with made-up data, the page with no script and nothing of the canvas's runtime (`support.js`, claude.ai's own, which is never copied here). GitHub Pages serves the pages from main's `/docs` at https://cquinones100.github.io/switchboard/design/, the kit's first board as `index.html` and every page beside a sidebar of them all; the sources are left out of the site (`docs/_config.yml`).

This is a project skill of Switchboard's repository (`.claude/skills/`), for working on Switchboard itself; the skills Switchboard ships are in `assets/skills/`, and this one isn't, so no Switchboard user gets it.

It needs Node and Google Chrome (`CHROME=<path>` names another Chrome or Chromium).

## 1. A worktree

Work in a worktree of this repository, as any change is: when the session is in the main clone, make one (`git worktree add .claude/worktrees/design-docs-<date> -b design-docs/<date> origin/main`) and work there.

## 2. Check and draw

Run, from the worktree:

```
node .claude/skills/design-docs/scripts/render.js sync docs/design/src docs/design
```

It first holds every board's text to the design's made-up names (`scripts/made-up.json`: people, handles, ticket prefixes, PR numbers, colors, channels, organizations and hosts), where its links, its resources and its CSS point included, since the page is published whole, every file of `docs/design/src/` whole (a board's script and data, `canvas.json`, and a source no page draws, in a folder or not), since they are published beside the pages, and to the words of `~/.config/switchboard/leak-check.txt`. When it finds anything else it exits 2 naming each board and the text, and draws nothing: stop and tell the user. A name that isn't made up is changed in the board's source, then the sync is run again; one that is made up and new goes into `made-up.json`, after the user says so, as a change of its own (below).

Then it writes each board `canvas.json` puts on its `design` page and the Index lists as `<number>.html` in `docs/design/` (the first, the kit's, as `index.html`; the Index board itself isn't a page, the sidebar taking its place), each board's link (`<Name>.dc.html`) pointing at that board's page, each shown beside a sidebar of every board grouped as the Index groups them (the page's own marked, its group open, the others folded) under a bar stepping to the boards before and after it, all with no script, removes the pages of boards gone from the sources, and rewrites `docs/design/README.md`: the site's address, where the sources are and the date, then the boards grouped as the Index groups them, each its number and title linking to its page on the site. Nothing changed, it changes nothing.

Look at the README's diff and at a few of the changed boards before committing, drawn with `node .claude/skills/design-docs/scripts/render.js docs/design/src/<Name>.dc.html <scratchpad>/<name>.png [width height]` (the Read tool shows a PNG): a board drawn blank, cut off or with its text off its meters is a renderer problem to fix here, not to land.

## 3. Push it

Commit `docs/design/` alone (the sources and the pages) with a message saying which boards changed, following CLAUDE.md, read the commits' diff by eye for anything that identifies the user or their employer, since the site is public, and then run, as this exact command:

```
node .claude/skills/design-docs/scripts/render.js push
```

It fetches main, refuses when main on origin has commits the branch lacks (rebase onto `origin/main`, then run it again) or when any commit it would push touches a path outside `docs/design/`, and runs `switchboard leak-check` on the commits, dropping what `made-up.json` explains in `docs/design/` (its ticket prefixes, PR numbers, and the colors leak-check reads as PR numbers). With nothing left it pushes `HEAD` to main, with no review loop and no question (CLAUDE.md's "Pushing", and `.githooks/pre-push` lets such a push through by the same rule); otherwise it prints each finding and exits 1: ask the user about each, and a new color it prints goes into `colors` once you have seen it is one. It exits 2 when leak-check couldn't look or git couldn't fetch: ask then too.

Once it has pushed, report a direct link to each changed board's page (`https://cquinones100.github.io/switchboard/design/<number>.html`, the kit's first board `index.html`), which the site shows once its build ends. Then remove the worktree and its branch, which have nothing left that main lacks.

A change to the renderer, `made-up.json`, this skill or anything else outside `docs/design/` is an ordinary change: commit it on its own and land it as `/queue-and-clean` does (read the file `switchboard skill-path queue-and-clean` prints and follow it), which runs the review loop and asks before pushing; `render.js push` refuses it. In that landing `node .claude/skills/design-docs/scripts/render.js made-up <range>` stands in for `switchboard leak-check <range>`, answering the made-up findings in `docs/design/` the same way.

## GitHub Pages

The site is GitHub Pages serving main's `/docs` folder (Jekyll renders `docs/guide.md` and this README there too, copies the pages as they are, and leaves `design/src` out, as `docs/_config.yml` says). When `gh api repos/cquinones100/switchboard/pages` answers 404, Pages is off: ask the user, since it publishes the site, and on a yes turn it on with `gh api -X POST repos/cquinones100/switchboard/pages -f 'source[branch]=main' -f 'source[path]=/docs'`, then check that https://cquinones100.github.io/switchboard/design/ serves the Index once its build ends.

## The renderer

`scripts/render.js` evaluates each board's script (its `renderVals()`) in a `node:vm` context of its own, with no `require`, `process` or file access, and expands the board's `<sc-for>` loops and `{{holes}}` into plain HTML under a Content-Security-Policy that runs no script and loads nothing but Google Fonts. A grid board (one whose rows sit in a `white-space: pre` container) loses the whitespace between its template's tags, which there would be a blank line pushing its text a row below its meters and pictures; the Slack boards and the Index keep theirs. `node scripts/render.js <board.dc.html> <out.png> [width height]` draws one board with Chrome headless at 1×; `node scripts/render.js --self-test` checks a grid fixture's page has no line breaks between its tags and a Slack fixture's keeps its spaces, the made-up-data check, a sync of the fixtures' sources into pages and a README, and, where Chrome is, draws both fixtures, checking each picture's size, that it isn't blank, and that the grid fixture's meter is on the very rows of the text it belongs to, so a one-row shift fails; `tests/it/design_docs.rs` runs it where Node is present, and checks `render.js push` and the pre-push hook in a repository of its own. It uses no npm packages.
