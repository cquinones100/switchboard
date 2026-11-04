// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's ticket board: the Linear issues assigned to you, as
// columns by where they stand: Todo, In progress, In review (a started state
// whose name says review), and Done in the last DONE_DAYS. Fetched every
// BOARD_POLL_MS in the background with one GraphQL query, and drawing reads
// only the last answer; a failed fetch keeps it. The key is the one the
// ticket scripts use: LINEAR_API_KEY, else the Keychain's linear-api-key,
// else worktree-from-ticket's config file's linearApiKey. Only while the
// linear and ticket_board settings are on (see views.js and the daemon).

const fs = require("node:fs");
const { count } = require("../requests");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { LINEAR } = require("./setup-help");

const BOARD_POLL_MS = 120_000;
const DONE_DAYS = 7;
const COLUMNS = ["Todo", "In progress", "In review", "Done"];
const CONFIG = path.join(os.homedir(), ".config/switchboard/linear.json");

const board = { columns: COLUMNS.map((title) => ({ title, items: [] })), loaded: false, error: undefined };

const columnOf = (issue) =>
  issue.state.type === "completed" ? "Done" : issue.state.type === "started" ? (/review/i.test(issue.state.name) ? "In review" : "In progress") : "Todo";

// A board card: the ticket and its title, then its state, priority and
// project, and the tab working on it when its name has the ticket, as
// tab-label.sh gives it. An urgent one needs you.
const itemOf = (issue, tabs) => {
  const tab = tabs.find((t) => new RegExp(`(^|[^a-z0-9])${issue.identifier}([^0-9]|$)`, "i").test(t.name ?? ""));
  const marks = [issue.state.name, issue.priority ? issue.priorityLabel : "", issue.project?.name, tab ? `tab ${tab.index}` : ""].filter(Boolean);
  return {
    title: `${issue.identifier} ${issue.title}`,
    detail: marks.join(" · "),
    state: issue.state.type === "completed" ? "idle" : issue.priority === 1 ? "attention" : "active",
    url: issue.url,
    ticket: issue.identifier,
    time: Date.parse(issue.completedAt ?? issue.updatedAt) || undefined,
    column: columnOf(issue),
    priority: issue.priority || 5,
  };
};

// Lays out the issues as the board: Todo by priority, urgent first, the rest
// most recently changed first.
const boardFrom = (issues, tabs = []) => {
  const columns = COLUMNS.map((title) => ({ title, items: [] }));
  issues.filter((issue) => issue?.identifier && issue.state).map((issue) => itemOf(issue, tabs)).forEach((item) => columns.find((c) => c.title === item.column).items.push(item));
  columns.forEach((c) => c.items.sort((a, b) => (c.title === "Todo" ? a.priority - b.priority : 0) || (b.time ?? 0) - (a.time ?? 0)));
  return columns;
};

const apiKey = () =>
  new Promise((resolve) => {
    if (process.env.LINEAR_API_KEY) return resolve(process.env.LINEAR_API_KEY);
    execFile("security", ["find-generic-password", "-s", "linear-api-key", "-w"], { encoding: "utf8", timeout: 5_000 }, (err, stdout) => {
      if (!err && stdout.trim()) return resolve(stdout.trim());
      try {
        resolve(JSON.parse(fs.readFileSync(CONFIG, "utf8")).linearApiKey || undefined);
      } catch {
        resolve(undefined);
      }
    });
  });

const QUERY = `query($since: DateTimeOrDuration!) {
  viewer { assignedIssues(first: 100, orderBy: updatedAt, filter: { or: [{ state: { type: { in: ["unstarted", "started"] } } }, { completedAt: { gte: $since } }] }) {
    nodes { identifier title url priority priorityLabel updatedAt completedAt state { name type } project { name } }
  } }
}`;

const fetchTickets = async (tabs, changed, log = () => {}) => {
  const key = await apiKey();
  board.loaded = true;
  if (!key) {
    board.error = LINEAR;
    return changed();
  }
  try {
    const since = new Date(Date.now() - DONE_DAYS * 86_400_000).toISOString();
    count("Linear", "GraphQL", "sidebar: tickets");
    const res = await fetch("https://api.linear.app/graphql", {
      method: "POST",
      headers: { "Content-Type": "application/json", Authorization: key },
      body: JSON.stringify({ query: QUERY, variables: { since } }),
      signal: AbortSignal.timeout(30_000),
    });
    const body = await res.json();
    if (body.errors?.length) throw new Error(body.errors[0].message);
    board.columns = boardFrom(body.data.viewer.assignedIssues.nodes, tabs);
    board.error = undefined;
  } catch (err) {
    board.error = String(err.message).split("\n")[0];
    log(`ticket board: ${board.error}`);
  }
  changed();
};

// Closing a ticket from mission control's menus: the state it would move to,
// its team's first completed state by the team's own order (Done, or what the
// team calls it), then the move. Each asks Linear with the same key.
const linear = async (query, variables) => {
  const key = await apiKey();
  if (!key) throw new Error("No Linear API key");
  count("Linear", "GraphQL", "sidebar: a ticket's state");
  const res = await fetch("https://api.linear.app/graphql", {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: key },
    body: JSON.stringify({ query, variables }),
    signal: AbortSignal.timeout(30_000),
  });
  const body = await res.json();
  if (body.errors?.length) throw new Error(body.errors[0].message);
  return body.data;
};
const doneStateOf = async (identifier) => {
  const data = await linear(`query($id: String!) { issue(id: $id) { id state { type } team { states(filter: { type: { eq: "completed" } }) { nodes { id name position } } } } }`, { id: identifier });
  const issue = data.issue;
  const state = [...(issue?.team?.states?.nodes ?? [])].sort((a, b) => a.position - b.position)[0];
  return issue && state && { issue: issue.id, already: issue.state?.type === "completed", state };
};
const closeTicket = async (issue, state) => {
  const data = await linear(`mutation($id: String!, $state: String!) { issueUpdate(id: $id, input: { stateId: $state }) { success } }`, { id: issue, state });
  return data.issueUpdate?.success === true;
};

// The view: the columns, and their items in column order for the cursor.
const ticketView = () => ({
  title: "Tickets",
  board: true,
  columns: board.columns,
  items: board.columns.flatMap((c) => c.items),
  loaded: board.loaded,
  error: board.error,
  builtIn: true,
});

module.exports = { TICKET_POLL_MS: BOARD_POLL_MS, TICKET_COLUMNS: COLUMNS, ticketsFrom: boardFrom, fetchTickets, ticketView, ticketBoard: board, apiKey, doneStateOf, closeTicket };
