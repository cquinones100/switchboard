// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's ticket board: the issues assigned to you, in Linear or
// Jira as the tracker setting says (lib/tracker), as columns by where they
// stand: Todo, In progress, In review (a started state whose name says
// review), and Done in the last DONE_DAYS. Fetched every BOARD_POLL_MS in the
// background, and drawing reads only the last answer; a failed fetch keeps it.
// The key is the one the ticket scripts use. Only while the tracker and
// ticket_board settings are on (see views.js and the daemon).

const { tracker } = require("../tracker");
const { trackerSetup } = require("./setup-help");

const BOARD_POLL_MS = 120_000;
const DONE_DAYS = 7;
const COLUMNS = ["Todo", "In progress", "In review", "Done"];

const board = { columns: COLUMNS.map((title) => ({ title, items: [] })), loaded: false, error: undefined };

// A started state whose name says review, or Jira's "Pull Request", is In
// review.
const columnOf = (issue) =>
  issue.state.type === "completed" ? "Done" : issue.state.type === "started" ? (/review|pull request/i.test(issue.state.name) ? "In review" : "In progress") : "Todo";

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

const fetchTickets = async (tabs, changed, log = () => {}) => {
  const provider = tracker();
  board.loaded = true;
  if (!provider) return changed();
  if (!(await provider.credentials())) {
    board.error = trackerSetup(provider);
    return changed();
  }
  try {
    const since = new Date(Date.now() - DONE_DAYS * 86_400_000).toISOString();
    board.columns = boardFrom(await provider.assigned(since), tabs);
    board.error = undefined;
  } catch (err) {
    board.error = String(err.message).split("\n")[0];
    log(`ticket board: ${board.error}`);
  }
  changed();
};

// Closing a ticket from mission control's menus: the state it would move to,
// then the move (Linear's team's first completed state, or Jira's first
// transition into Done), each asked of the tracker.
const doneStateOf = (identifier) => tracker().doneStateOf(identifier);
const closeTicket = (issue, state) => tracker().closeTicket(issue, state);

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

module.exports = { TICKET_POLL_MS: BOARD_POLL_MS, TICKET_COLUMNS: COLUMNS, ticketsFrom: boardFrom, fetchTickets, ticketView, ticketBoard: board, doneStateOf, closeTicket };
