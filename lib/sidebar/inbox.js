// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Inbox view: your unread Linear notifications that need
// you, newest first: someone mentioning you, in a ticket or a comment, a new
// comment on a ticket of yours, and a ticket newly assigned to you. Status
// changes and the like, most of Linear's inbox, are left out. Fetched every
// INBOX_POLL_MS (INBOX_SLOW_MS with the poll_fast setting off) in the background, and when you go to the view, with the key
// the ticket scripts use; drawing reads only the last answer. Each one also
// counts in the sidebar's notifications. Enter offers what the Tickets board
// does with its ticket, and marks the notification read in Linear. Only while
// the linear and view_inbox settings are on.

const { apiKey } = require("./ticketboard");
const { count } = require("../requests");
const { LINEAR } = require("./setup-help");

const INBOX_POLL_MS = 15_000;
const INBOX_SLOW_MS = 120_000;
const KINDS = {
  issueMention: "mentioned you",
  issueCommentMention: "mentioned you in a comment",
  issueNewComment: "commented",
  issueAssignedToYou: "assigned it to you",
};

const inbox = { items: [], loaded: false, error: undefined };

const QUERY = `query { notifications(first: 100) { nodes { id type createdAt readAt archivedAt snoozedUntilAt actor { name }
  ... on IssueNotification { issue { identifier title url } comment { body url } } } } }`;

const linear = async (key, query, variables = {}) => {
  count("Linear", "GraphQL", "sidebar: inbox");
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

// The notifications as the view lists them.
const inboxFrom = (nodes, now = Date.now()) =>
  nodes
    .filter((n) => KINDS[n.type] && n.issue && !n.readAt && !n.archivedAt && !(Date.parse(n.snoozedUntilAt ?? "") > now))
    .sort((a, b) => Date.parse(b.createdAt) - Date.parse(a.createdAt))
    .map((n) => ({
      title: `${n.issue.identifier} ${n.issue.title}`,
      detail: [`${n.actor?.name ?? "Someone"} ${KINDS[n.type]}`, n.comment?.body?.replace(/\s+/g, " ").slice(0, 160)].filter(Boolean).join(": "),
      state: n.type === "issueNewComment" ? "active" : "attention",
      time: Date.parse(n.createdAt),
      url: n.comment?.url ?? n.issue.url,
      ticket: n.issue.identifier,
      notification: n.id,
    }));

const fetchInbox = async (changed = () => {}, log = () => {}) => {
  const key = await apiKey();
  inbox.loaded = true;
  if (!key) {
    inbox.error = LINEAR;
    return changed();
  }
  try {
    inbox.items = inboxFrom((await linear(key, QUERY)).notifications.nodes);
    inbox.error = undefined;
  } catch (err) {
    inbox.error = `Couldn't read your Linear inbox: ${String(err.message).split("\n")[0]}`;
    log(inbox.error);
  }
  changed();
};

// Marks one read in Linear, and takes it off the view at once.
const markRead = async (id) => {
  const key = await apiKey();
  if (!key) return "No Linear API key.";
  try {
    await linear(key, `mutation($id: String!, $at: DateTime!) { notificationUpdate(id: $id, input: { readAt: $at }) { success } }`, { id, at: new Date().toISOString() });
    inbox.items = inbox.items.filter((i) => i.notification !== id);
    return undefined;
  } catch (err) {
    return `Couldn't mark it read: ${String(err.message).split("\n")[0]}`;
  }
};

const inboxView = () => ({ title: "Inbox", inbox: true, items: inbox.items, loaded: inbox.loaded, error: inbox.error, builtIn: true });

module.exports = { INBOX_POLL_MS, INBOX_SLOW_MS, inbox, inboxFrom, fetchInbox, markRead, inboxView };
