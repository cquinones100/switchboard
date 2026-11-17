# The design of UX 2.0

The boards of the UX 2.0 design, "Switchboard UX", as web pages: each one a screen of the workspace (160 × 45 cells) or of Slack on a phone, with made-up data. GitHub Pages serves them at https://cquinones100.github.io/switchboard/design/, starting from the kit's first board, every page beside a sidebar of them all. Drawn from the boards' sources in [`src/`](https://github.com/cquinones100/switchboard/tree/main/docs/design/src) on 2026-11-17 by the `design-docs` project skill (`.claude/skills/design-docs/`), which writes every page again: a board is changed in its source, never in its page.

## The kit and the index

- [0. The kit: the atoms every board is drawn with](https://cquinones100.github.io/switchboard/design/index.html)
- [0c. The kit's board: columns of cards, one look for PRs, Tickets and Autopilot](https://cquinones100.github.io/switchboard/design/0c.html)
- [0d. The kit: a person, one avatar for Slack, Linear and GitHub, Large beside their words and Small inline](https://cquinones100.github.io/switchboard/design/0d.html)
- [45. Layout: how a view fills a screen of any size, every board being 160 × 45](https://cquinones100.github.io/switchboard/design/45.html)
- [61. A lone pane on your 365 × 91 screen, A: the Work map capped at 165 columns and centered, the spare columns sunken on both sides](https://cquinones100.github.io/switchboard/design/61.html)
- [62. A lone pane on your 365 × 91 screen, B: the Work map capped at 165 columns against the sidebar, the spare columns sunken on its right](https://cquinones100.github.io/switchboard/design/62.html)
- [77. Beside a lone pane on your 365 × 91 screen: what can open in the spare columns, centered there, each a key and a click](https://cquinones100.github.io/switchboard/design/77.html)
- [63. Two panes on your 365 × 91 screen: a tab with Notes beside it splits the whole width, past a lone pane's cap](https://cquinones100.github.io/switchboard/design/63.html)

## The workspace

A tab and what it shows in Claude's place.

- [1. The workspace: the sidebar with notifications, the tab in focus and its views](https://cquinones100.github.io/switchboard/design/1.html)
- [75. A pinned tab (M-f): first in Needs you, the flag ⚑ in place of its state's mark](https://cquinones100.github.io/switchboard/design/75.html)
- [2. M-a: straight to what needs you, answered in Claude's own dialog](https://cquinones100.github.io/switchboard/design/2.html)
- [4. M-\\: two tabs side by side](https://cquinones100.github.io/switchboard/design/4.html)
- [6. A tab's history](https://cquinones100.github.io/switchboard/design/6.html)
- [12. M-d: the diff in Claude's place](https://cquinones100.github.io/switchboard/design/12.html)
- [13. M-v: the PR in Claude's place](https://cquinones100.github.io/switchboard/design/13.html)
- [14. The ticket in Claude's place, its view's actions in the m menu](https://cquinones100.github.io/switchboard/design/14.html)
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
- [79. A tab waiting for a slot of the queue for heavy work: ◌ in its sidebar row and its card, its place in line](https://cquinones100.github.io/switchboard/design/79.html)
- [5. Landing as a pipeline](https://cquinones100.github.io/switchboard/design/5.html)
- [16. Autopilot as a kanban board](https://cquinones100.github.io/switchboard/design/16.html)
- [29. Routines: a prompt of yours on a schedule, run headless with the tools it allows, opening no tab; a run that needed another tool failed](https://cquinones100.github.io/switchboard/design/29.html)
- [30. A new routine: its name, directory, schedule, allowed tools and prompt in one box](https://cquinones100.github.io/switchboard/design/30.html)
- [76. Enter on a routine whose run failed for a tool: allow it and run again, open its session in a tab, or edit it](https://cquinones100.github.io/switchboard/design/76.html)
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
- [21. Slack: what is new for you, by section as Slack's sidebar lists them; the section one row per thread, the chosen thread whole beside it](https://cquinones100.github.io/switchboard/design/21.html)
- [53. Slack relay: which messages reach you, and where](https://cquinones100.github.io/switchboard/design/53.html)
- [22. Inbox: Linear's notifications](https://cquinones100.github.io/switchboard/design/22.html)
- [23. Notifications: the sidebar's, whole](https://cquinones100.github.io/switchboard/design/23.html)
- [11. Several views at once](https://cquinones100.github.io/switchboard/design/11.html)

## Mission control · system

Spend, processes, settings and keys.

- [24. Usage: spend, limits and requests](https://cquinones100.github.io/switchboard/design/24.html)
- [25. Resources: what each tab's processes take](https://cquinones100.github.io/switchboard/design/25.html)
- [80. Resources: each kind of slot, how many are held, by which tab and for how long, and who waits in order](https://cquinones100.github.io/switchboard/design/80.html)
- [26. Settings: every option by category, your files' too](https://cquinones100.github.io/switchboard/design/26.html)
- [27. Keys: every binding, yours over the defaults](https://cquinones100.github.io/switchboard/design/27.html)

## Proposal: mission control · factory

- [87. Factory: Spend, every dollar today, Claude's, Codex's, the hidden Haiku's and CI's, against a budget that pauses autopilot](https://cquinones100.github.io/switchboard/design/87.html)
- [88. Factory: a tab over its hourly limit, its row's menu (and its notification's): open it, pause it, or pause autopilot and every routine](https://cquinones100.github.io/switchboard/design/88.html)
- [89. Factory: Limits, when the 5-hour and weekly limits run out at today's pace, the services' rate limits and every request](https://cquinones100.github.io/switchboard/design/89.html)
- [90. Factory: Landing, the lock and its queue now, then how landings, review loops and CI went this week](https://cquinones100.github.io/switchboard/design/90.html)

## Moving between mission control's views

Five ways to show its groups and views, to pick one.

- [64. Mission control's views with Settings' "Keep the tabs sidebar in mission control" off: while it is open, the sidebar lists its groups and views](https://cquinones100.github.io/switchboard/design/64.html)
- [65. With "Keep the tabs sidebar in mission control" on: mission control's own column of groups and views, the tabs sidebar beside it](https://cquinones100.github.io/switchboard/design/65.html)
- [66. Dropped: one row, the group you are in open with its views, the others folded to what needs you](https://cquinones100.github.io/switchboard/design/66.html)
- [67. Dropped: a home screen, a tile for every view saying what it would show, Enter opening one](https://cquinones100.github.io/switchboard/design/67.html)
- [68. Dropped: a path for a bar, and M-Space again opening every view in a box to type into](https://cquinones100.github.io/switchboard/design/68.html)

## Notes

Items, the tabs working them and their plans.

- [7. Notes, each item showing the tab working it](https://cquinones100.github.io/switchboard/design/7.html)
- [8. A note item's plan, and its choice, beside it](https://cquinones100.github.io/switchboard/design/8.html)
- [69. Notes' list: Recent first, the last 8 notes opened, then Folders opening on Notes, no All Notes or tags](https://cquinones100.github.io/switchboard/design/69.html)
- [70. All 20 › under Recent, a click or Enter: every note opened lately, the last 20, in the finder's box](https://cquinones100.github.io/switchboard/design/70.html)
- [83. Notes draws a Mermaid block as its diagram while the cursor is outside it: subgraphs boxed with their title on the edge, one too wide cut with ›, a pie kept as code](https://cquinones100.github.io/switchboard/design/83.html)
- [84. The cursor in a Mermaid block shows its code to edit; the Format menu's Show diagram code keeps the note's diagrams as code until Show diagrams](https://cquinones100.github.io/switchboard/design/84.html)
- [85. A note another tab has open: read-only here, the bar naming that tab with Go to it lit, the toolbar dim, ✎ in the list and the sidebar, a tip the first time](https://cquinones100.github.io/switchboard/design/85.html)
- [86. Once the other tab lets go: the note becomes editable in place, a green line saying so, the cursor and toolbar back](https://cquinones100.github.io/switchboard/design/86.html)

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
- [78. M-F: search everything, a chip per source to search, its count once it answers](https://cquinones100.github.io/switchboard/design/78.html)
- [91. M-F with the chosen result previewed beside the results: its place, then 5 lines each side of the match, the match lit; under the results in a narrow box](https://cquinones100.github.io/switchboard/design/91.html)

## Core and extras

What a newcomer starts on and turns on.

- [37. Extras: each bundle one switch, with what it adds and what it needs](https://cquinones100.github.io/switchboard/design/37.html)
- [38. Core only: the workspace as a newcomer first sees it](https://cquinones100.github.io/switchboard/design/38.html)
- [39. Core only: mission control's Tabs, its views Work and System alone](https://cquinones100.github.io/switchboard/design/39.html)
- [40. The first run: switchboard setup ends by naming the extras](https://cquinones100.github.io/switchboard/design/40.html)
- [41. Switchboard's first open: one tab of Claude, and the welcome tip](https://cquinones100.github.io/switchboard/design/41.html)
- [42. A tip suggesting an extra the first time it fits: a tab's Claude opens a PR](https://cquinones100.github.io/switchboard/design/42.html)

## Slack on a phone

The relay's DM, threads and Home tab.

- [46. Slack on a phone: the DM, the welcome and each tab's thread header, its alerts kept in its thread](https://cquinones100.github.io/switchboard/design/46.html)
- [47. A tab's thread on a phone: a permission prompt answered with buttons](https://cquinones100.github.io/switchboard/design/47.html)
- [48. The Home tab on a phone, grouped as the sidebar](https://cquinones100.github.io/switchboard/design/48.html)
- [49. A tab's screen, opened from Home](https://cquinones100.github.io/switchboard/design/49.html)
- [50. Proposed: Home with Next, the desktop's state colors and today's usage](https://cquinones100.github.io/switchboard/design/50.html)
- [51. Proposed: landing in the tab's thread, queue-and-clean's steps and its push question as buttons](https://cquinones100.github.io/switchboard/design/51.html)
- [52. Proposed: autopilot's settings as a Slack form that also starts it](https://cquinones100.github.io/switchboard/design/52.html)
- [81. Proposed: autopilot on the Home tab, working, paused by you and paused by its spend limit, edited in place](https://cquinones100.github.io/switchboard/design/81.html)
- [82. Proposed: jump from Home to a tab's thread, its name and a Thread › button opening it at the newest message](https://cquinones100.github.io/switchboard/design/82.html)

## Autopilot's queue outside the note

- [92. Beside a lone pane: a new row, Autopilot's queue, opening the tab's repository's queue in the spare columns](https://cquinones100.github.io/switchboard/design/92.html)
- [93. The queue as a pane right of the tab: To do across every repository in the order autopilot takes them, each item's repository, and Done with its Done line](https://cquinones100.github.io/switchboard/design/93.html)
- [94. Enter on a queued item: make its choice, open its plan, work on it now, move it, what it waits on, park it, copy its ID](https://cquinones100.github.io/switchboard/design/94.html)
- [95. Move…: the item lifts and follows the arrows or a drag; Enter puts it down, Esc puts it back](https://cquinones100.github.io/switchboard/design/95.html)
- [96. Waits on…: every open item in every queue, a switch each, in place of After: lines](https://cquinones100.github.io/switchboard/design/96.html)
- [97. Make its choice…: the plan's question and options as its note words them, the recommended one first](https://cquinones100.github.io/switchboard/design/97.html)
- [98. Mission control's Autopilot from every repository's queue: each card its ID, place and repository, Parked under the board, Landed with its Done line](https://cquinones100.github.io/switchboard/design/98.html)
- [99. A plan note is the item: its bar names its ID, place, holds and what it needs, Enter opening the queue's menu; ticking an option makes the choice](https://cquinones100.github.io/switchboard/design/99.html)
- [100. Slack's Home tab lists the queue, an overflow menu on each item, and Make its choice… as a form](https://cquinones100.github.io/switchboard/design/100.html)
- [101. Autopilot's settings across every repository: their order, which it takes from first, each one's switch, cap of tabs and spend limit beside the shared ones, and Add a repository…](https://cquinones100.github.io/switchboard/design/101.html)
- [102. Enter on a repository in autopilot's settings: move it, turn it off, its cap, its spend limit, where it lands, its queue](https://cquinones100.github.io/switchboard/design/102.html)
- [103. Moving a repository while autopilot runs: Next up rearranged across the queues as it is put down](https://cquinones100.github.io/switchboard/design/103.html)
- [104. Add a repository… in autopilot's settings: your repositories not yet in it, or a folder; it joins last, on, with a cap and spend of its own](https://cquinones100.github.io/switchboard/design/104.html)

## The diff's totals

- [105. M-d with the whole change's totals at the head of its file list, as GitHub's PR header: files, lines added and deleted, five cells shared out by them](https://cquinones100.github.io/switchboard/design/105.html)
- [106. The diff's file list filtered by path with /: the totals count only the files left, of how many in all](https://cquinones100.github.io/switchboard/design/106.html)

## Proposal: replying from the slack view

- [107. Enter on a Slack item with Settings' "Reply from the Slack view" on (off by default): the replies that apply, each naming where it goes](https://cquinones100.github.io/switchboard/design/107.html)
- [108. The reply box: where it goes, the message it answers quoted, what you type, Send and Cancel](https://cquinones100.github.io/switchboard/design/108.html)
- [109. Send asks first, naming the destination, \[Y/n\]; nothing is sent before a yes](https://cquinones100.github.io/switchboard/design/109.html)
- [110. Sent: the reply in the thread pane as yours, marked sent, the thread's count up by one](https://cquinones100.github.io/switchboard/design/110.html)

## Boxes beside a lone pane

- [111. Beside a lone pane, a box about the tab (here M-u's links picker) drawn as a pane: the spare columns' full height, a header line naming it and the tab, the rule at the pane's edge its only border, no shadow; M-p, M-F and the pickers stay in the middle](https://cquinones100.github.io/switchboard/design/111.html)
- [112. A notification while the links picker is beside a lone pane: raised in the middle of the whole screen with its shadow, as every message, question and notification is; the pane beside dims and keeps its place](https://cquinones100.github.io/switchboard/design/112.html)

## Proposal: autopilot's blocked column

- [113. Autopilot's Blocked column: an item held by an After: line on an open item stands between Next and Working, its card naming each item it waits on, the mark of where that one stands and its tab's number; drawn only while any is blocked](https://cquinones100.github.io/switchboard/design/113.html)
- [114. Enter on a blocked card: a choice per item it waits on leads its menu, going to the tab working it or, with none, opening its plan](https://cquinones100.github.io/switchboard/design/114.html)

## Proposal: notifications by source

- [115. Notifications by where they come from: a heading per source (its view, else its app) with its count, the sources ordered by their newest; a source of more than 5 split by channel, repository, tab or routine; a folded source its heading alone; the headings in the keycaps' place](https://cquinones100.github.io/switchboard/design/115.html)
- [116. Enter on a source's heading: fold it, or dismiss every notification of it the sidebar shows, after asking](https://cquinones100.github.io/switchboard/design/116.html)
- [117. The sidebar's notifications, one line a source: its mark, its name, its count and its newest one's words; Enter opens the Notifications view on that source](https://cquinones100.github.io/switchboard/design/117.html)

## The diff's buttons and its own settings

- [118. The diff's row of buttons as the kit draws Switchboard's own: bold in the accent on the panel surface between rounded ends, on the chosen surface under the mouse or reached by Tab](https://cquinones100.github.io/switchboard/design/118.html)
- [119. switchboard-diff run alone: its own header, and a Settings button before Close (also M-m) that it has only when standalone](https://cquinones100.github.io/switchboard/design/119.html)
- [120. The diff's own settings view (M-m, or Settings lit): its options as Switchboard's Settings rows draw them, and when one is out, a dim 0.2.0 is out · switchboard-diff update under the title and a row to update](https://cquinones100.github.io/switchboard/design/120.html)

## The diff's story as a map

- [121. In Map mode, M-d opens on the story as a map: a box a chapter, the goal at the top, an arrow from each to what it builds on, dashed where the code can't confirm the link, read chapters marked, the stories' tab bar over it](https://cquinones100.github.io/switchboard/design/121.html)
- [122. A chapter opened on its own map: the functions, types and tables it adds or changes, an arrow from each to what it uses, over its files and hunks; Enter on a thing goes to its hunk](https://cquinones100.github.io/switchboard/design/122.html)
- [123. A hunk a second chapter can't be read without, shown whole there under a dim also in chapter 5 line that goes to its home; its file listed in both chapters](https://cquinones100.github.io/switchboard/design/123.html)
- [124. The stories' tab bar on Side work: what the main story can be read without, its own map; Loading outlines a thread of its own; \[s \]s or a click between them](https://cquinones100.github.io/switchboard/design/124.html)
- [125. Files mode, today's diff unchanged: the Map / Files switch leading the row of buttons on Files (g m and g f too), the diff opening in the mode used last](https://cquinones100.github.io/switchboard/design/125.html)

## Proposal: which note is the tab's

- [126. Which note is this tab's (M-e with none): every note newest first with its age, at most three suggestions above under a label, and typing keeps the matches newest first, written matches before scattered ones](https://cquinones100.github.io/switchboard/design/126.html)

## A tab following several prs

- [127. A tab following every PR its session opens: the current one's number first in its label, +2 after its name, each PR's state under it in the sidebar, M-v naming which of them it shows](https://cquinones100.github.io/switchboard/design/127.html)
- [128. Its card in the Tabs view: the current PR's number in its title, its PRs under what it is doing, the current one first, each one's repository when not the tab's and where it stands](https://cquinones100.github.io/switchboard/design/128.html)
- [129. Show another of this tab's PRs (M-v's and M-d's menus, M-p): a picker as every list is, each PR's repository, title and state, the current one marked; Enter makes the chosen one current](https://cquinones100.github.io/switchboard/design/129.html)

## Marks for review loop and queue-and-clean

- [130. Marks for the sidebar's Review loop and Queue-and-clean groups, three options beside today's shared spinner: A turning (◐ and △▲), B still (◎ and ⏏), C still (⌕ and ⇡), each in a row and its group header](https://cquinones100.github.io/switchboard/design/130.html)

## Proposal: the pr and ticket conversations in boxes

- [131. The PR view's conversation in boxes, as GitHub draws it: the author's avatar beside a box a comment, its header bar naming who, what and when, a line down the side joining them; a bot's long body folded; a review comment with its line of code; the checks, reviewers and files beside it](https://cquinones100.github.io/switchboard/design/131.html)
- [132. The same at 80 columns: no panel, its facts one line under the head; the avatar Small in each box's header bar, the line down the side at the boxes' left](https://cquinones100.github.io/switchboard/design/132.html)
- [133. The ticket view (M-i) with the same atom: the description as its first box, each comment in its own, a reply inside its thread's box; sub-issues and the facts beside it as before](https://cquinones100.github.io/switchboard/design/133.html)
