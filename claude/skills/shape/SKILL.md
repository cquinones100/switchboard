---
name: shape
description: Shape a Linear or Jira ticket with the user. Read the code first, check the ticket tracker for tickets that already cover the work, draft the ticket in the team's usual form, and create or update it once the user approves. Given an existing ticket, asks whether to run a review loop on it or keep shaping it. Hands off to /wt afterwards.
argument-hint: "[idea, or ticket ID or URL and what to do with it]"
disable-model-invocation: true
---

# Shape

Turn a rough idea into a ticket someone can pick up and build: clear on the problem, grounded in the code as it is, honest about scope. The `shape` command in `bin/` starts a session with this skill; it also works in a session that is already running.

Tickets live in Linear or Jira, whichever Switchboard's tracker setting names. Before anything else, read [tracker.md](tracker.md) beside this file and settle which one, as it says; then every step below that says "the tracker" means that one, with the words that file maps for it (a Linear team is a Jira project, a Linear project a Jira epic, a sub-issue a sub-task).

Arguments: a description of the problem or idea, or a ticket ID or URL (`ABC-123`, or a Linear link with `/issue/ABC-123/` or a Jira one with `/browse/ABC-123` in it) to work on an existing ticket. Text after a ticket (and after `--continue`, when that is there) is the user's direction for this shaping, such as what to change or look into: keep it in mind throughout, and let it decide between a review loop and shaping where it says which. With neither, ask what the ticket is about. Given a ticket, read it first and then ask what to do with it (see "Given a ticket" below).

## Ground rules

- Read only. Do not edit, create, or delete files in the repository, and do not commit, branch, or push. Shaping ends with a ticket, not a change; `/wt` starts the work.
- Answer what the code can answer yourself. Ask the user about intent, priority, and scope, not about how the code works today.
- Nothing is written to the tracker until the user approves a draft.

## Name the tab

In tmux, the tab says what is being shaped, the way `/wt` names a ticket's tab, so it is easy to find in the tab bar, `M-j` and Slack. Name it as soon as the skill starts, from the arguments: `Shaping: <the first four or five words>` for a description, or `Shaping <ID>` for a ticket. Name it again once there is a ticket, from its ID and title: after reading the ticket you were given, and after creating one in step 8, as `Shaping <ID>: <title>`. `/wt` clears the name when it hands the tab over, and the tab then takes the ticket's own name.

Only when `$TMUX_PANE` is set, and with the name cut to 60 characters and kept to letters, digits, spaces and `. , : - ( ) /` (drop anything else, quotes included, since it goes into a shell command):

```
tmux set-option -w -t "$TMUX_PANE" @tab_name 'Shaping ABC-123: Show which phase is active' &&
  ~/.config/tmux/window-names.sh "$(tmux display-message -p -t "$TMUX_PANE" '#{session_name}')"
```

This sets the same `@tab_name` that renaming the tab in `M-p` and `bin/shape` set, and renames the tabs straight away rather than on the next focus. If it fails, carry on: a name is no reason to stop.

## Given a ticket

When the argument names an existing ticket, fetch it and its comments from the tracker before anything else. If it doesn't exist or can't be read, say so and stop. Name the tab for it (see "Name the tab"). When `--continue` follows the ticket (as `shape --continue`, `shape-batch --continue` and the palette's "Continue shaping tickets" pass it), give the two-line summary below and carry straight on as "Continue shaping it", without asking. Otherwise give a two-line summary of where it stands (its title, state, and whether it already has a problem statement, an approach and acceptance criteria) and ask which of these to do, with `AskUserQuestion`:

- **Run a review loop on it.** Hand the ticket to the repository's `/review-loop` skill in ticket mode, which has Codex review the ticket's text against the code as it is (see that skill and its `references/ticket-mode.md`). Use it when the ticket looks ready and you want it checked before anyone builds it. If this repository has no `review-loop` skill, say so and continue shaping instead. When the loop finishes, show its findings and ask whether to fold them into the ticket by continuing below from step 6; nothing is written to the tracker without approval, as always.
- **Continue shaping it.** Carry on with the steps below, refining this ticket rather than drafting a new one. Use it when the ticket is still rough, or the user wants to change its scope.

Put the one the ticket's state suggests first, marked as recommended: a review loop for a ticket that already reads as complete, shaping for one that doesn't.

## A checklist item from a note

The notes action (`tmux/note-shape.sh`, through `shape-batch`) passes one item from a checklist in the user's notes, on one line: the item, `(from my note "<title>")`, then `Details:` with everything indented under it that isn't a checklist item (lines joined by ` / `), then `Sub-items, each its own ticket:` with the unticked checklist items indented under it, numbered, each with its own details in brackets. The details are part of the item's ticket. Each sub-item is a ticket of its own.

When there are sub-items, ask first, before anything else but naming the tab, with `AskUserQuestion`, how to organise them:

- **A project:** "<item>" becomes a Linear project (a Jira epic), and each sub-item a ticket in it.
- **A ticket with sub-issues:** "<item>" becomes a ticket, and each sub-item a sub-issue (a Jira sub-task) of it.

Then run the steps for the item and for each sub-item together: step 2 checks each one for a ticket that already covers it (and, for a project, for a project that already covers the item), and a sub-item one covers is offered as that sub-item rather than drafted again. Step 7 drafts all of them at once, the project or parent first, and step 8 creates the project or parent, then each sub-item in it or under it, linking the ones that already existed instead of copying them. An item that names a ticket ID is checked against that ticket first.

## Steps

1. **Settle the team.** Run `~/.config/tmux/tracker.sh config`: its `team` is the team key (Linear) or project key (Jira) to file in, such as `ABC`, and `labels` the labels for a new ticket. They come from `linearTeam` and `linearLabels` in `~/.config/switchboard/linear.json`, or `jiraProject` and `jiraLabels` in `jira.json`, both in Switchboard's Settings under Linear and Jira. If the team or project is missing, take the one of the ticket being refined, or ask, and suggest adding it to that file. Keep team names and keys out of this repository: it is public.

2. **Check whether a ticket already covers it.** Given a description rather than a ticket, search the tracker first, before reading any code: open tickets in the team, and ones closed in the last 90 days (tracker.md has the JQL for Jira), by the description's key words. A ticket covers it when it is about the same change, not merely the same area. If one or more do, show each (its ID, title, state and link, and a line on why it matches) and ask with `AskUserQuestion` whether to resume shaping one of them, with an option per ticket, up to three, and **Shape a new ticket** last. Resuming one makes it the ticket being refined: fetch it and its comments, name the tab for it (see "Name the tab"), and carry on from step 3 as "Continue shaping it" would, without asking the "Given a ticket" question. If nothing covers it, say so in a line and go on. Shaping several items at once, as `shape-batch` and the notes action do, each session checks its own item.

3. **Learn the team's ticket shape.** Read three or four recent tickets from that team, preferring ones that were built and closed. Note their headings and conventions (acceptance criteria, labels) and use them, but the user's own rules for tickets in `~/.claude/CLAUDE.md` (a section such as "PRs and Linear tickets", whichever tracker) win where they differ: business logic first, a diagram for a pipeline, implementation details last and brief.

4. **Understand the problem in the code.** Find where the behaviour lives, how it works now, and what a change would touch. Name the files and functions you looked at, so the draft points at real places. When refining an existing ticket, read it and its comments first, and check whether anything it assumes has changed since it was written.

5. **Look for related tickets.** With the code understood, search the tracker again for open and recently closed tickets that overlap with this or that it depends on, which step 2's search for the same work can't find. If one turns out to cover it after all, say so and ask whether to refine that one instead of opening another.

6. **Ask only what is left.** Usually a few questions about intent and scope: what the user wants to be true afterwards, and what is deliberately out. Keep them to what changes the ticket.

7. **Draft.** Show the full ticket, as the user's rules for tickets in `~/.claude/CLAUDE.md` say: a title that says what changes, then the business logic (what changes and why, answering what a teammate would later ask), a diagram for a pipeline or multi-step flow, acceptance criteria, what is out of scope, and last the implementation in a few words, linking to the code on GitHub rather than describing it. Use step 3's headings and conventions where they don't conflict. Note related tickets as links rather than copying them in. Then wait for the user's approval or edits. Revise until they approve.

8. **Write it to the tracker.** Once the user approves the draft, create the ticket in the team, with its labels if set, or update the ticket being refined, as tracker.md's "Writing" says. For a note item with sub-items, create the project (in the team) or the parent ticket first, then each sub-item as an issue in that project or a sub-issue of that ticket, as "A checklist item from a note" says. Link the related tickets from step 5. Name the tab for a ticket just created, or again if the title changed (see "Name the tab"). Reply with the ticket ID and URL.

9. **Review the created ticket.** Right after creating it, run the repository's `/review-loop` in ticket mode on the new ticket, without asking: the user set this up in this skill, so it counts as asking for the loop by name. It reads the ticket from the tracker and ends asking whether to save its edits there, as ticket mode always does. For a note item with sub-items, review the project or parent and each sub-item, one loop each. If the repository has no `review-loop` skill, or its working tree isn't clean, say so and skip it. Only a ticket this session created gets the loop, not one being refined. Then offer `/wt <ID>` to start the work in this tab.
