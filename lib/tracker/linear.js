// Linear, for lib/tracker: every question the workspace asks Linear, in one
// place, each answered in the shape Linear's GraphQL API gives it, which is
// also the shape lib/tracker/jira.js answers in, so the scripts that use them
// needn't know which one they asked.
//
// The key is a personal API key (https://linear.app/settings/api), from
// LINEAR_API_KEY, else the Keychain (service linear-api-key), else
// linearApiKey in ~/.config/switchboard/linear.json. It is sent as
// the Authorization header as it is, with no scheme before it.

const { configValue, keychain } = require("./config");

// LINEAR_API_URL is for the tests, which stand in for Linear on localhost.
const API = process.env.LINEAR_API_URL || "https://api.linear.app/graphql";

const missing =
  "Linear isn't set up: this needs a Linear API key. In Linear, open Settings, Security & access, and make a personal API key, then store it in the Keychain: " +
  'run security add-generic-password -U -s linear-api-key -a "$USER" -w in a terminal and paste the key at its prompt.';

const credentials = async () => process.env.LINEAR_API_KEY || (await keychain("linear-api-key")) || configValue("linear.json", "linearApiKey");

// The data of one query, or throws Linear's first error. A missing key throws
// too, with what to do about it.
const query = async (text, variables = {}, timeout = 30_000) => {
  const key = await credentials();
  if (!key) throw new Error(missing);
  const res = await fetch(API, {
    method: "POST",
    headers: { "Content-Type": "application/json", Authorization: key },
    body: JSON.stringify({ query: text, variables }),
    signal: AbortSignal.timeout(timeout),
  });
  const body = await res.json().catch(() => undefined);
  if (!body) throw new Error(`Linear answered ${res.status} ${res.statusText}`);
  if (body.errors?.length) {
    const err = new Error(body.errors[0].message);
    // What Linear says when there is no such ticket, as opposed to a failure.
    err.notFound = body.errors.some((e) => e.extensions?.code === "INPUT_ERROR" || /not found/i.test(e.message));
    throw err;
  }
  return body.data;
};

// One issue by its ID (ABC-123), or null when Linear has none.
const orNull = (promise) => promise.catch((err) => (err.notFound ? { issue: null } : Promise.reject(err)));

const ISSUE = "identifier title url branchName state { name type } project { name url }";

const issue = async (id) => (await orNull(query(`query($id: String!) { issue(id: $id) { ${ISSUE} } }`, { id }))).issue;

// Many issues at once, the ones Linear has: one query per hundred rather
// than one per ticket, keyed by nothing, in no particular order.
const CHUNK = 100;
const issues = async (ids) => {
  const chunks = [];
  for (let i = 0; i < ids.length; i += CHUNK) chunks.push(ids.slice(i, i + CHUNK));
  const pages = await Promise.all(
    chunks.map(async (batch) => {
      const filters = batch
        .map((id) => /^([A-Za-z][A-Za-z0-9]*)-(\d+)$/.exec(id))
        .filter(Boolean)
        .map(([, key, number]) => ({ and: [{ number: { eq: Number(number) } }, { team: { key: { eq: key.toUpperCase() } } }] }));
      const data = await query(`query($filters: [IssueFilter!]) { issues(filter: { or: $filters }, first: ${CHUNK + 50}) { nodes { ${ISSUE} } } }`, { filters });
      return data.issues?.nodes ?? [];
    }),
  );
  return pages.flat();
};

// Everything the ticket view and image show, or null.
const DETAIL = `query($id: String!) { issue(id: $id) {
  identifier title url description createdAt updatedAt priorityLabel estimate
  state { name type } assignee { name } creator { name } project { name } cycle { number name }
  labels { nodes { name } } parent { identifier title }
  children { nodes { identifier title state { name } } }
  relations { nodes { type relatedIssue { identifier title state { name } } } }
  attachments { nodes { title url } }
  comments(first: 100) { nodes { id body createdAt parent { id } user { name } botActor { name } } }
} }`;
const detail = async (id) => (await orNull(query(DETAIL, { id }, 15_000))).issue;

// The project an issue is in, { name, url }, or null; undefined when there
// is no such issue.
const project = async (id) => {
  const found = await issue(id);
  return found === null ? undefined : (found.project ?? null);
};

// Assigned to you, open or done since <since>, most recently changed first.
const assigned = async (since) =>
  (
    await query(
      `query($since: DateTimeOrDuration!) {
  viewer { assignedIssues(first: 100, orderBy: updatedAt, filter: { or: [{ state: { type: { in: ["unstarted", "started"] } } }, { completedAt: { gte: $since } }] }) {
    nodes { identifier title url priority priorityLabel updatedAt completedAt state { name type } project { name } }
  } }
}`,
      { since },
    )
  ).viewer.assignedIssues.nodes;

const search = async (term, limit = 20) =>
  (await query(`query($q: String!) { searchIssues(term: $q, first: ${limit}) { nodes { identifier title url state { name } } } }`, { q: term })).searchIssues.nodes;

// For the standup note: issues assigned to you or made by you changed since
// <since>, with their state moves, and the ones you have started.
const standup = (since) =>
  query(
    `query($since: DateTimeOrDuration!) {
  changed: issues(first: 100, filter: { or: [{ assignee: { isMe: { eq: true } } }, { creator: { isMe: { eq: true } } }], updatedAt: { gte: $since } }) { nodes { identifier title url
    history(first: 50) { nodes { createdAt fromState { name } toState { name } } } } }
  started: issues(first: 50, filter: { assignee: { isMe: { eq: true } }, state: { type: { eq: "started" } } }) { nodes { identifier title url state { name } } } }`,
    { since },
  );

// Closing a ticket: the state it would move to, its team's first completed
// state by the team's own order (Done, or what the team calls it), then the
// move.
const doneStateOf = async (id) => {
  const data = await query(`query($id: String!) { issue(id: $id) { id state { type } team { states(filter: { type: { eq: "completed" } }) { nodes { id name position } } } } }`, { id });
  const found = data.issue;
  const state = [...(found?.team?.states?.nodes ?? [])].sort((a, b) => a.position - b.position)[0];
  return found && state && { issue: found.id, already: found.state?.type === "completed", state };
};
const closeTicket = async (issueId, stateId) =>
  (await query(`mutation($id: String!, $state: String!) { issueUpdate(id: $id, input: { stateId: $state }) { success } }`, { id: issueId, state: stateId })).issueUpdate?.success === true;

// The inbox: notifications, and marking one read.
const notifications = async () =>
  (
    await query(`query { notifications(first: 100) { nodes { id type createdAt readAt archivedAt snoozedUntilAt actor { name }
  ... on IssueNotification { issue { identifier title url } comment { body url } } } } }`)
  ).notifications.nodes;
const markRead = (id) => query(`mutation($id: String!, $at: DateTime!) { notificationUpdate(id: $id, input: { readAt: $at }) { success } }`, { id, at: new Date().toISOString() });

// A ticket's page, from the workspace in config, without asking Linear.
const urlOf = (id) => {
  const workspace = process.env.LINEAR_WORKSPACE || configValue("linear.json", "linearWorkspace");
  return workspace ? `https://linear.app/${workspace}/issue/${id}` : undefined;
};

// Where /shape and the other skills file a new ticket: the workspace's page,
// the team and the labels from linear.json, never the key.
const filing = () => {
  const workspace = process.env.LINEAR_WORKSPACE || configValue("linear.json", "linearWorkspace");
  return { site: workspace ? `https://linear.app/${workspace}` : undefined, team: configValue("linear.json", "linearTeam"), labels: configValue("linear.json", "linearLabels") ?? [] };
};

module.exports = {
  key: "linear",
  name: "Linear",
  missing,
  credentials,
  query,
  issue,
  issues,
  detail,
  project,
  assigned,
  search,
  standup,
  doneStateOf,
  closeTicket,
  notifications,
  markRead,
  urlOf,
  filing,
};
