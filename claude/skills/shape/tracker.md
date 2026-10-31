# The ticket tracker

Tickets live in Linear or Jira (Cloud), whichever Switchboard's `tracker` setting names, one per machine. Every skill that reads or writes a ticket settles which one first and uses only that one, through Claude's connector for it: the Linear connector, or the Atlassian connector for Jira. `/shape`, `/note-tickets`, `/queue-and-clean`, `/deploy-test` and `/project-status` read this file (`/wt` only hands an ID to `wt`, which asks the tracker itself), so a change to how a tracker is used belongs here.

## Which one

Run `~/.config/tmux/tracker.sh name`. It prints `Linear` or `Jira`, and exits 2 when the setting is off: then say tickets are turned off in Switchboard's Settings (`M-p`, Settings, "Ticket tracker") and leave out the ticket part, or stop when the skill is about nothing else. When the script isn't there (Switchboard's skills are set up but not its tmux part), run `node ~/switchboard/lib/tracker/cli.js` with the same command wherever this file or a skill says `~/.config/tmux/tracker.sh` (`name`, `config`, `show <ID>`): it answers the same, and Switchboard's permissions allow it too.

## The same thing in each

| Linear | Jira |
| --- | --- |
| Team, `linearTeam` in `~/.config/switchboard/linear.json` (a key like `ABC`) | Project, `jiraProject` in `~/.config/switchboard/jira.json` (a key like `ABC`) |
| Labels for a new ticket, `linearLabels` | `jiraLabels` |
| Project | Epic |
| Sub-issue | Sub-task (a sub-task issue type, its parent the ticket), or a child of an epic |
| State type `started` | Status category In Progress |
| State type `completed` | Status category Done |
| State type `canceled` | Status category Done whose resolution or status reads as Won't Do, Duplicate, Cancelled, Declined, Rejected or Obsolete, as Switchboard's Jira provider counts them |
| "Related" relation | Issue link of the type named Relates |
| `https://linear.app/<linearWorkspace>/issue/<ID>` | `<jiraSite>/browse/<ID>` |
| Project milestones and status updates | None: an epic's child issues, its fix versions and its comments say where it stands |

`~/.config/tmux/tracker.sh config` prints those settings as JSON: `tracker`, `site` (the Jira site, or the Linear workspace's page), `team` (the Linear team or Jira project key) and `labels`. Read them that way, never from the files, which may hold a key or token; Switchboard's permissions allow every `tracker.sh` command, since they only read. Keep team and project keys, labels, site names and ticket IDs out of the Switchboard repository: it is public.

## Reading

- **Linear:** the Linear connector (`get_issue`, `list_issues`, `list_comments`, `list_projects` and the rest).
- **Jira:** the Atlassian connector. Its calls take a `cloudId`: call `getAccessibleAtlassianResources` once and take the site whose URL is the `site` `tracker.sh config` prints; with no `site` there and more than one site listed, ask which, and suggest setting it (`jiraSite`, in Settings under Jira). One ticket is `getJiraIssue` (view `evidence` for its issue links, parent and custom fields, and `responseContentFormat: "markdown"`), its comments `listJiraIssueComments`, and any search `searchJiraIssuesUsingJql`. Anything else read-only is found with `discover` and run with `executeRead`, which Switchboard's permissions allow: the reads named below (`listJiraIssueTransitions`, `listJiraProjectIssueTypesMetadata`, `getJiraIssueTypeMetaWithFields`, `listJiraIssueLinkTypes`) are run that way, not as tools of their own.
- **Either, for one ticket's text:** `~/.config/tmux/tracker.sh show <ID>` prints the ticket as JSON in Linear's shape (`.data.issue`: title, state, description in Markdown, comments, project, links), with Switchboard's own key or token and no connector. It exits 3 when there is no such ticket.

JQL for the searches the skills make, with the project key `tracker.sh config` gives as `team`:

- Open tickets about some words: `project = ABC AND statusCategory != Done AND text ~ "phase label" ORDER BY updated DESC`
- Closed in the last 90 days: `project = ABC AND statusCategory = Done AND statusCategoryChangedDate >= -90d AND text ~ "phase label"`
- Recent ones that were built: `project = ABC AND statusCategory = Done ORDER BY statusCategoryChangedDate DESC`, skipping any the table above counts as canceled
- Yours in progress: `assignee = currentUser() AND statusCategory = "In Progress"`
- An epic's tickets: `parent = ABC-12`

Done is when the ticket reached the Done category (`statusCategoryChangedDate`), not `resolved` or `resolution`, which a workflow may never set. `text ~` matches words, not a phrase in order, and a word Jira can't search for (a stop word, a symbol) is dropped, so search with the few words that name the thing.

## Writing

Every write is the user's to approve: show what will be written and wait, as the skill says, and never write a ticket to try something out, since the board is shared with the user's team.

- **Linear:** the Linear connector (`save_issue`, `create_comment` and the rest).
- **Jira:** the Atlassian connector's write operations, run with `executeWrite` as `{ name, cloudId, inputs }`, `cloudId` beside `name`, never inside `inputs`: `createJiraIssue`, `editJiraIssue`, `addOrEditJiraIssueComment`, `createJiraIssueLink` and `transitionJiraIssue`. Ask `discover` for the one you need first, for its inputs; on a connection where `discover` lists no write operations, call it by its name all the same, and follow the `repairHint` an error gives. Claude Code asks the user before each one when the connector's write tools need approval; that is how it should be. A write refused with "Insufficient scope" naming `write:jira:agent-interface` means the connector's sign-in only granted reading: the user disconnects and connects Atlassian again in claude.ai's Settings, Connectors, and accepts write access on Atlassian's consent screen (and `/mcp` here), or, if it offers none, their organization has blocked writes.
  - **A new ticket:** `createJiraIssue` with `projectKey`, `issueType` (its name), `summary`, `description`, `contentFormat: "markdown"` and, in an epic or under a ticket, `parent` (its key). In the `jiraProject` project, with `jiraLabels` if set, the description in Markdown and the issue type the project's recent tickets use for this kind of work (Story, Task, Bug), from `listJiraProjectIssueTypesMetadata`. When creating fails on a missing field, `getJiraIssueTypeMetaWithFields` names the required ones; ask the user for any value you can't tell.
  - **A project of tickets:** an epic, then each ticket with the epic as its `parent`. **A ticket with sub-issues:** the ticket, then each sub-item as a sub-task with it as `parent`.
  - **Related tickets:** `createJiraIssueLink` with `linkType: "Relates"`, `inwardIssue` and `outwardIssue`, one call per pair (`listJiraIssueLinkTypes` lists the site's link types).
  - **Done:** `listJiraIssueTransitions`, then `transitionJiraIssue` with the transition whose target status is in the Done category and isn't one the table above counts as canceled, taking the first when there are several. When none leads to Done from where the ticket is, say so rather than walk it through other statuses.
