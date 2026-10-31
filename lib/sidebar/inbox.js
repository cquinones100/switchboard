// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Inbox view: your unread Linear notifications that need
// you, newest first: someone mentioning you, in a ticket or a comment, a new
// comment on a ticket of yours, and a ticket newly assigned to you. Status
// changes and the like, most of Linear's inbox, are left out. Fetched every
// INBOX_POLL_MS in the background, and when you go to the view, with the key
// the ticket scripts use; drawing reads only the last answer. Each one also
// counts in the sidebar's notifications. Enter offers what the Tickets board
// does with its ticket, and marks the notification read in Linear. Only while
// the tracker setting is Linear (Jira has no inbox to read) and view_inbox is
// on.

const linear = require("../tracker/linear");
const { trackerKey } = require("../tracker");
const { isOn } = require("../settings");
const { trackerSetup } = require("./setup-help");

const INBOX_POLL_MS = 120_000;
const KINDS = {
  issueMention: "mentioned you",
  issueCommentMention: "mentioned you in a comment",
  issueNewComment: "commented",
  issueAssignedToYou: "assigned it to you",
};

const inbox = { items: [], loaded: false, error: undefined };

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
  inbox.loaded = true;
  if (!(await linear.credentials())) {
    inbox.error = trackerSetup(linear);
    return changed();
  }
  try {
    inbox.items = inboxFrom(await linear.notifications());
    inbox.error = undefined;
  } catch (err) {
    inbox.error = `Couldn't read your Linear inbox: ${String(err.message).split("\n")[0]}`;
    log(inbox.error);
  }
  changed();
};

// Marks one read in Linear, and takes it off the view at once.
const markRead = async (id) => {
  try {
    await linear.markRead(id);
    inbox.items = inbox.items.filter((i) => i.notification !== id);
    return undefined;
  } catch (err) {
    return `Couldn't mark it read: ${String(err.message).split("\n")[0]}`;
  }
};

// Whether the Inbox is asked for and shown: Linear's alone.
const inboxOn = () => trackerKey() === "linear" && isOn("view_inbox");

const inboxView = () => ({ title: "Inbox", inbox: true, items: inbox.items, loaded: inbox.loaded, error: inbox.error, builtIn: true });

module.exports = { INBOX_POLL_MS, inbox, inboxFrom, fetchInbox, markRead, inboxOn, inboxView };
