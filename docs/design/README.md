# The design of UX 2.0

The boards of the UX 2.0 design, "Switchboard UX", as web pages: each one a screen of the workspace (160 × 45 cells) or of Slack on a phone, with made-up data. GitHub Pages serves them at https://cquinones100.github.io/switchboard/design/, starting from the kit's first board, every page beside a sidebar of them all. Drawn from the boards' sources in [`src/`](https://github.com/cquinones100/switchboard/tree/main/docs/design/src) on 2026-11-13 by the `design-docs` project skill (`.claude/skills/design-docs/`), which writes every page again: a board is changed in its source, never in its page.

## The kit and the index

- [0. The kit: the atoms every board is drawn with](https://cquinones100.github.io/switchboard/design/index.html)
- [0c. The kit's board: columns of cards, one look for PRs, Tickets and Autopilot](https://cquinones100.github.io/switchboard/design/0c.html)
- [0d. The kit: a person, one avatar for Slack, Linear and GitHub, Large beside their words and Small inline](https://cquinones100.github.io/switchboard/design/0d.html)
- [45. Layout: how a view fills a screen of any size, every board being 160 × 45](https://cquinones100.github.io/switchboard/design/45.html)
- [61. A lone pane on your 365 × 91 screen, A: the Work map capped at 165 columns and centered, the spare columns sunken on both sides](https://cquinones100.github.io/switchboard/design/61.html)
- [62. A lone pane on your 365 × 91 screen, B: the Work map capped at 165 columns against the sidebar, the spare columns sunken on its right](https://cquinones100.github.io/switchboard/design/62.html)
- [63. Two panes on your 365 × 91 screen: a tab with Notes beside it splits the whole width, past a lone pane's cap](https://cquinones100.github.io/switchboard/design/63.html)

## The workspace

A tab and what it shows in Claude's place.

- [1. The workspace: the sidebar with notifications, the tab in focus and its views](https://cquinones100.github.io/switchboard/design/1.html)
- [2. M-a: straight to what needs you, answered in Claude's own dialog](https://cquinones100.github.io/switchboard/design/2.html)
- [4. M-\\: two tabs side by side](https://cquinones100.github.io/switchboard/design/4.html)
- [6. A tab's history](https://cquinones100.github.io/switchboard/design/6.html)
- [12. M-d: the diff in Claude's place](https://cquinones100.github.io/switchboard/design/12.html)
- [13. M-v: the PR in Claude's place](https://cquinones100.github.io/switchboard/design/13.html)
- [14. M-i: the ticket in Claude's place](https://cquinones100.github.io/switchboard/design/14.html)
- [15. M-p: the palette, every command by what it does](https://cquinones100.github.io/switchboard/design/15.html)
- [43. The sidebar filtered to one repository, the rest folded under Other repos](https://cquinones100.github.io/switchboard/design/43.html)
- [44. Filtering the sidebar to a repository: M-p's picker, Every repository to clear](https://cquinones100.github.io/switchboard/design/44.html)
- [56. A build running without blocking: the sidebar's build row, and its steps a click opens](https://cquinones100.github.io/switchboard/design/56.html)
- [57. Back and forward: ← and → first on the sidebar's top line, the mouse over ← naming the tab it goes to](https://cquinones100.github.io/switchboard/design/57.html)
- [58. A right-click on ← or →: the whole trail in a menu, any step one choice away](https://cquinones100.github.io/switchboard/design/58.html)
- [59. The mission control button: ⊞ after ← → on the sidebar's top line, the count that needs you beside it, the mouse over it naming it](https://cquinones100.github.io/switchboard/design/59.html)
- [60. A right-click on ⊞: mission control's views by group, any one a choice away](https://cquinones100.github.io/switchboard/design/60.html)

## Mission control · work

The tabs and where each piece of work stands.

- [3. M-Space: mission control's Tabs: what each tab asks, is doing or did, rather than its screen; a finished tab, loops and reminders too](https://cquinones100.github.io/switchboard/design/3.html)
- [5. Landing as a pipeline](https://cquinones100.github.io/switchboard/design/5.html)
- [16. Autopilot as a kanban board](https://cquinones100.github.io/switchboard/design/16.html)
- [29. Routines: a prompt of yours on a schedule, in the tab its last run used](https://cquinones100.github.io/switchboard/design/29.html)
- [30. A new routine: its name, directory, schedule, tab and prompt in one box](https://cquinones100.github.io/switchboard/design/30.html)
- [17. Work map, replaced by Reopen (18): every piece of work, with a tab or without](https://cquinones100.github.io/switchboard/design/17.html)
- [18. Reopen: closed tabs, past sessions and worktrees no tab has open, in one view, in place of the Work map](https://cquinones100.github.io/switchboard/design/18.html)
- [71. Enter on a closed tab in Reopen: reopen it, its note, its directory, or forget it](https://cquinones100.github.io/switchboard/design/71.html)
- [72. Enter on a past session in Reopen: resume it, its diff, its PR, or remove its worktree after asking](https://cquinones100.github.io/switchboard/design/72.html)
- [73. Enter on a worktree with no tab in Reopen: open a tab and resume its Claude, its diff, lazygit, or remove it after asking](https://cquinones100.github.io/switchboard/design/73.html)
- [74. Enter on Reopen's folded merged worktrees: show them, or remove them all after asking](https://cquinones100.github.io/switchboard/design/74.html)

## Mission control · code

PRs, tickets and CI.

- [10. PRs as a board, the description beside it](https://cquinones100.github.io/switchboard/design/10.html)
- [19. Tickets as a board, the ticket beside it](https://cquinones100.github.io/switchboard/design/19.html)
- [20. CI: every run, failures first](https://cquinones100.github.io/switchboard/design/20.html)

## Mission control · day

The calendar, reminders, Slack and what came in.

- [9. Calendar: the day as a timeline, the event beside it](https://cquinones100.github.io/switchboard/design/9.html)
- [28. Reminders: every tab's reminder on a month, the months ahead under it](https://cquinones100.github.io/switchboard/design/28.html)
- [21. Slack: what is new for you, by section as Slack's sidebar lists them, newest first in each](https://cquinones100.github.io/switchboard/design/21.html)
- [53. Slack relay: which messages reach you, and where](https://cquinones100.github.io/switchboard/design/53.html)
- [22. Inbox: Linear's notifications](https://cquinones100.github.io/switchboard/design/22.html)
- [23. Notifications: the sidebar's, whole](https://cquinones100.github.io/switchboard/design/23.html)
- [11. Several views at once](https://cquinones100.github.io/switchboard/design/11.html)

## Mission control · system

Spend, processes, settings and keys.

- [24. Usage: spend, limits and requests](https://cquinones100.github.io/switchboard/design/24.html)
- [25. Resources: what each tab's processes take](https://cquinones100.github.io/switchboard/design/25.html)
- [26. Settings: every option by category, your files' too](https://cquinones100.github.io/switchboard/design/26.html)
- [27. Keys: every binding, yours over the defaults](https://cquinones100.github.io/switchboard/design/27.html)

## Proposed: moving between mission control's views

Five ways to show its groups and views, to pick one.

- [64. Mission control's views, option 1: while it is open, the sidebar lists its groups and views with what each says](https://cquinones100.github.io/switchboard/design/64.html)
- [65. Option 2: mission control's own column of groups and views, as Slack's sidebar, the tabs' sidebar kept](https://cquinones100.github.io/switchboard/design/65.html)
- [66. Option 3: one row, the group you are in open with its views, the others folded to what needs you](https://cquinones100.github.io/switchboard/design/66.html)
- [67. Option 4: a home screen, a tile for every view saying what it would show, Enter opening one](https://cquinones100.github.io/switchboard/design/67.html)
- [68. Option 5: a path for a bar, and M-Space again opening every view in a box to type into](https://cquinones100.github.io/switchboard/design/68.html)

## Notes

Items, the tabs working them and their plans.

- [7. M-N: Notes, each item showing the tab working it](https://cquinones100.github.io/switchboard/design/7.html)
- [8. A note item's plan, and its choice, beside it](https://cquinones100.github.io/switchboard/design/8.html)
- [69. Notes' list: Recent first, the last 8 notes opened, then Folders opening on Notes, no All Notes or tags](https://cquinones100.github.io/switchboard/design/69.html)
- [70. All 20 › under Recent, a click or Enter: every note opened lately, the last 20, in the finder's box](https://cquinones100.github.io/switchboard/design/70.html)

## Boxes

The dialogs the views open, each over its view.

- [31. M-j: the tab picker, and M-\\ picking the tab to show beside](https://cquinones100.github.io/switchboard/design/31.html)
- [32. Several views at once, from any view's m menu: which views, their order and layout](https://cquinones100.github.io/switchboard/design/32.html)
- [33. Finding a key, from the Keys view's m menu: press it to find its binding](https://cquinones100.github.io/switchboard/design/33.html)
- [34. Setting or changing a tab's reminder: when, typed or picked, and what for](https://cquinones100.github.io/switchboard/design/34.html)
- [35. Autopilot's settings: one list moved with ↑↓, a pause button, what the chosen row does said once, changes applied as made](https://cquinones100.github.io/switchboard/design/35.html)
- [36. Working on a note's item: its repository, the branch it lands on, or autopilot's queue](https://cquinones100.github.io/switchboard/design/36.html)
- [54. Enter in any mission control view: the chosen item's actions in a menu, here a calendar event's](https://cquinones100.github.io/switchboard/design/54.html)
- [55. m in any mission control view: the view's own actions in a menu, here Calendar's](https://cquinones100.github.io/switchboard/design/55.html)

## Core and extras

What a newcomer starts on and turns on.

- [37. Extras: each bundle one switch, with what it adds and what it needs](https://cquinones100.github.io/switchboard/design/37.html)
- [38. Core only: the workspace as a newcomer first sees it](https://cquinones100.github.io/switchboard/design/38.html)
- [39. Core only: mission control's Tabs, its bar Work and System alone](https://cquinones100.github.io/switchboard/design/39.html)
- [40. The first run: switchboard setup ends by naming the extras](https://cquinones100.github.io/switchboard/design/40.html)
- [41. Switchboard's first open: one tab of Claude, and the welcome tip](https://cquinones100.github.io/switchboard/design/41.html)
- [42. A tip suggesting an extra the first time it fits: a tab's Claude opens a PR](https://cquinones100.github.io/switchboard/design/42.html)

## Slack on a phone

The relay's DM, threads and Home tab.

- [46. Slack on a phone: the DM, one line for each thing that needs you](https://cquinones100.github.io/switchboard/design/46.html)
- [47. A tab's thread on a phone: a permission prompt answered with buttons](https://cquinones100.github.io/switchboard/design/47.html)
- [48. The Home tab on a phone, grouped as the sidebar](https://cquinones100.github.io/switchboard/design/48.html)
- [49. A tab's screen, opened from Home](https://cquinones100.github.io/switchboard/design/49.html)
- [50. Proposed: Home with Next, the desktop's state colors and today's usage](https://cquinones100.github.io/switchboard/design/50.html)
- [51. Proposed: landing in the tab's thread, queue-and-clean's steps and its push question as buttons](https://cquinones100.github.io/switchboard/design/51.html)
- [52. Proposed: autopilot's settings as a Slack form that also starts it](https://cquinones100.github.io/switchboard/design/52.html)
