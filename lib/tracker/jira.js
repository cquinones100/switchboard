// Jira Cloud, for lib/tracker: the same questions lib/tracker/linear.js
// answers, asked of Jira's REST API (v3) and answered in Linear's shape, so
// the scripts that use them needn't know which tracker they asked. Where the
// two differ:
//
// - A state's type comes from its status category: To Do is unstarted, In
//   Progress started, Done completed, or canceled when its resolution or the
//   status's own name says so (Won't Do, Duplicate, Declined, Cancelled), since
//   a workflow may set no resolution.
// - A Linear project is a Jira epic: an issue's project is its parent epic.
// - Descriptions and comments are Jira's document format, made Markdown by
//   lib/tracker/adf.js.
// - Priorities run Highest (1, urgent) to Lowest (4), as Linear's do.
// - Jira has no inbox to read, so there is no notifications().
//
// It signs in with an API token (https://id.atlassian.com/manage-profile/security/api-tokens)
// and the email of the account that made it. The token is JIRA_API_TOKEN, else
// the Keychain (service jira-api-token), else jiraApiToken in
// ~/.config/switchboard/jira.json; the email JIRA_EMAIL, else
// jiraEmail there; the site JIRA_SITE, else jiraSite there, as
// https://<yours>.atlassian.net.

const { configValue, keychain } = require("./config");
const { markdownOf } = require("./adf");

const TOKENS = "https://id.atlassian.com/manage-profile/security/api-tokens";
// Stored from the clipboard through security's stdin, since its hidden prompt
// (-w with nothing after it) keeps only 128 characters, and Atlassian's
// tokens are longer.
const STORE = `printf 'add-generic-password -U -a "%s" -s jira-api-token -w "%s"\\n' "$USER" "$(pbpaste)" | security -i`;
const missing =
  `Jira isn't set up. Make an API token at ${TOKENS}, copy it, store it in the Keychain with: ${STORE}, ` +
  'and add "jiraSite": "https://<yours>.atlassian.net" and "jiraEmail" to ~/.config/switchboard/jira.json.';

const site = () => {
  const value = process.env.JIRA_SITE || configValue("jira.json", "jiraSite");
  if (!value) return undefined;
  return (/^https?:\/\//.test(value) ? value : `https://${value}`).replace(/\/+$/, "");
};

const credentials = async () => {
  const token = process.env.JIRA_API_TOKEN || (await keychain("jira-api-token")) || configValue("jira.json", "jiraApiToken");
  const email = process.env.JIRA_EMAIL || configValue("jira.json", "jiraEmail");
  const base = site();
  return token && email && base ? { site: base, email, token } : undefined;
};

// Jira's answer to one request, or throws. A missing issue throws with
// notFound set; a refused token says it may have expired, since Atlassian's
// tokens do after at most a year.
const request = async (method, path, body, timeout = 30_000) => {
  const creds = await credentials();
  if (!creds) throw new Error(missing);
  const res = await fetch(`${creds.site}${path}`, {
    method,
    headers: {
      Accept: "application/json",
      ...(body ? { "Content-Type": "application/json" } : {}),
      Authorization: `Basic ${Buffer.from(`${creds.email}:${creds.token}`).toString("base64")}`,
    },
    body: body ? JSON.stringify(body) : undefined,
    signal: AbortSignal.timeout(timeout),
  });
  if (res.status === 204) return {};
  const answer = await res.json().catch(() => undefined);
  if (res.ok) return answer ?? {};
  if (res.status === 401) {
    // A current token (ATATT…) ends in = and a checksum; one stored through
    // the Keychain's hidden prompt stops at 128 characters without them.
    if (/^ATATT/.test(creds.token) && creds.token.length === 128 && !creds.token.includes("=")) {
      throw new Error(`Jira refused the API token, which was cut off at 128 characters as it was stored. Copy it again and store it with: ${STORE}`);
    }
    throw new Error(`Jira refused the API token: it may have expired. Make a new one at ${TOKENS}.`);
  }
  const err = new Error(answer?.errorMessages?.[0] ?? Object.values(answer?.errors ?? {})[0] ?? `Jira answered ${res.status} ${res.statusText}`);
  err.notFound = res.status === 404;
  throw err;
};

// Jira answers a request it can't sign in as it does one for an issue that
// isn't there, so before an issue is taken to be missing, it checks once
// that the token works, and says so if it doesn't.
let signedIn = false;
const orNull = (promise) =>
  promise.catch(async (err) => {
    if (!err.notFound) throw err;
    if (!signedIn) {
      await request("GET", "/rest/api/3/myself");
      signedIn = true;
    }
    return null;
  });

const urlOf = (id) => {
  const base = site();
  return base ? `${base}/browse/${id}` : undefined;
};

const stateOf = (fields) => {
  const category = fields.status?.statusCategory?.key;
  const type =
    category === "done"
      ? /won.?t|cancel|duplicate|declin|reject|obsolete/i.test(`${fields.resolution?.name ?? ""} ${fields.status?.name ?? ""}`)
        ? "canceled"
        : "completed"
      : category === "indeterminate"
        ? "started"
        : "unstarted";
  return { name: fields.status?.name ?? "Unknown", type };
};

const PRIORITY = { highest: 1, blocker: 1, critical: 1, high: 2, major: 2, medium: 3, low: 4, minor: 4, lowest: 4, trivial: 4 };
const priorityOf = (fields) => {
  const name = fields.priority?.name;
  return { priority: PRIORITY[name?.toLowerCase()] ?? 0, priorityLabel: name ?? "No priority" };
};

// The parent epic, as Linear's project; a sub-task's parent is a task, not one.
const epicOf = (fields) => {
  const parent = fields.parent;
  if (!parent) return null;
  const type = parent.fields?.issuetype;
  return type?.hierarchyLevel === 1 || /epic/i.test(type?.name ?? "") ? { name: parent.fields?.summary ?? parent.key, url: urlOf(parent.key) } : null;
};

const FIELDS = ["summary", "status", "resolution", "parent", "priority", "updated", "resolutiondate", "statuscategorychangedate"];

const brief = (issue) => {
  const f = issue.fields ?? {};
  return {
    identifier: issue.key,
    title: f.summary ?? "",
    url: urlOf(issue.key),
    branchName: null,
    state: stateOf(f),
    project: epicOf(f),
    ...priorityOf(f),
    updatedAt: f.updated,
    completedAt: f.status?.statusCategory?.key === "done" ? (f.resolutiondate ?? f.statuscategorychangedate ?? null) : null,
  };
};

const issue = async (id) => {
  const found = await orNull(request("GET", `/rest/api/3/issue/${encodeURIComponent(id)}?fields=${FIELDS.join(",")}`));
  return found && brief(found);
};

// Many at once, the ones Jira has, a hundred to a request.
const CHUNK = 100;
const issues = async (ids) => {
  const chunks = [];
  for (let i = 0; i < ids.length; i += CHUNK) chunks.push(ids.slice(i, i + CHUNK));
  const pages = await Promise.all(
    chunks.map(async (batch) => (await request("POST", "/rest/api/3/issue/bulkfetch", { issueIdsOrKeys: batch, fields: FIELDS })).issues ?? []),
  );
  return pages.flat().map(brief);
};

// JQL, every page of it up to <limit> issues.
const jql = async (query, { fields = FIELDS, limit = 100, expand } = {}) => {
  const found = [];
  let nextPageToken;
  do {
    const page = await request("POST", "/rest/api/3/search/jql", {
      jql: query,
      fields,
      maxResults: Math.min(100, limit - found.length),
      ...(expand ? { expand } : {}),
      ...(nextPageToken ? { nextPageToken } : {}),
    });
    found.push(...(page.issues ?? []));
    nextPageToken = page.isLast === false ? page.nextPageToken : undefined;
  } while (nextPageToken && found.length < limit);
  return found;
};

// A date and time as JQL reads it, in your own time zone, as Jira takes it.
const jqlTime = (iso) => {
  const d = new Date(iso);
  const two = (n) => String(n).padStart(2, "0");
  return `"${d.getFullYear()}-${two(d.getMonth() + 1)}-${two(d.getDate())} ${two(d.getHours())}:${two(d.getMinutes())}"`;
};

const person = (user) => (user ? { name: user.displayName ?? user.emailAddress ?? "Someone" } : null);

const DETAIL_FIELDS = ["summary", "description", "status", "resolution", "priority", "assignee", "reporter", "creator", "created", "updated", "labels", "parent", "subtasks", "issuelinks", "issuetype"];

// Everything the ticket view and image show, in the shape Linear's detail
// query gives, or null. An epic's sub-issues are the issues under it, which
// Jira keeps as its children rather than its sub-tasks.
const detail = async (id) => {
  const key = encodeURIComponent(id);
  const found = await orNull(request("GET", `/rest/api/3/issue/${key}?fields=${DETAIL_FIELDS.join(",")}`, undefined, 15_000));
  if (!found) return null;
  const [comments, links] = await Promise.all([
    request("GET", `/rest/api/3/issue/${key}/comment?maxResults=100&orderBy=created`, undefined, 15_000).catch(() => ({ comments: [] })),
    request("GET", `/rest/api/3/issue/${key}/remotelink`, undefined, 15_000).catch(() => []),
  ]);
  const f = found.fields ?? {};
  const isEpic = f.issuetype?.hierarchyLevel >= 1 || /epic/i.test(f.issuetype?.name ?? "");
  const under = isEpic ? await jql(`parent = ${found.key} ORDER BY rank`, { fields: ["summary", "status"], limit: 100 }).catch(() => []) : [];
  const small = (i) => ({ identifier: i.key, title: i.fields?.summary ?? "", state: i.fields?.status ? { name: i.fields.status.name } : null });
  const parent = f.parent && !epicOf(f) ? { identifier: f.parent.key, title: f.parent.fields?.summary ?? "" } : null;
  return {
    tracker: "Jira",
    identifier: found.key,
    title: f.summary ?? "",
    url: urlOf(found.key),
    description: markdownOf(f.description),
    createdAt: f.created,
    updatedAt: f.updated,
    ...priorityOf(f),
    estimate: null,
    state: stateOf(f),
    assignee: person(f.assignee),
    creator: person(f.reporter ?? f.creator),
    project: epicOf(f),
    cycle: null,
    labels: { nodes: (f.labels ?? []).map((name) => ({ name })) },
    parent,
    children: { nodes: [...(f.subtasks ?? []), ...under].map(small) },
    relations: {
      nodes: (f.issuelinks ?? []).map((link) => {
        const other = link.outwardIssue ?? link.inwardIssue;
        const phrase = (link.outwardIssue ? link.type?.outward : link.type?.inward) ?? link.type?.name ?? "related to";
        return { type: phrase.replace(/^./, (c) => c.toUpperCase()), relatedIssue: small(other ?? {}) };
      }),
    },
    attachments: { nodes: (Array.isArray(links) ? links : []).filter((l) => l.object?.url).map((l) => ({ title: l.object.title || l.object.url, url: l.object.url })) },
    comments: {
      nodes: (comments.comments ?? []).map((c) => ({ id: c.id, body: markdownOf(c.body), createdAt: c.created, parent: null, user: person(c.author), botActor: null })),
    },
  };
};

// The epic an issue is in, { name, url }, or null; undefined when there is
// no such issue.
const project = async (id) => {
  const found = await issue(id);
  return found === null ? undefined : found.project;
};

// Assigned to you, open or finished in the last week, most recently changed
// first. <since> is when that week began.
const assigned = async (since) =>
  (await jql(`assignee = currentUser() AND (statusCategory != Done OR updated >= ${jqlTime(since)}) ORDER BY updated DESC`)).map(brief);

// Jira's text search, which reads the summary, description and comments. Its
// query language gives meaning to most punctuation, so only words are kept.
// A ticket key searched for comes first, since text search may not find it.
const search = async (term, limit = 20) => {
  const words = term.replace(/[^\p{L}\p{N}\s'-]+/gu, " ").replace(/\s+/g, " ").trim();
  const byKey = /^[A-Za-z][A-Za-z0-9]*-\d+$/.test(term.trim()) ? await issue(term.trim().toUpperCase()).catch(() => null) : null;
  const found = words ? (await jql(`text ~ "${words.replace(/"/g, "")}" ORDER BY updated DESC`, { limit })).map(brief) : [];
  return [...(byKey ? [byKey] : []), ...found.filter((i) => i.identifier !== byKey?.identifier)].slice(0, limit);
};

// For the standup note, as Linear's answers it: issues assigned to you or
// reported by you changed since <since>, with their status moves from the
// changelog, and the ones you have in progress.
const standup = async (since) => {
  const [changed, started] = await Promise.all([
    jql(`(assignee = currentUser() OR reporter = currentUser()) AND updated >= ${jqlTime(since)} ORDER BY updated DESC`, { expand: "changelog" }),
    jql("assignee = currentUser() AND statusCategory = \"In Progress\" ORDER BY updated DESC", { limit: 50 }),
  ]);
  return {
    changed: {
      nodes: changed.map((i) => ({
        ...brief(i),
        history: {
          nodes: (i.changelog?.histories ?? []).flatMap((h) =>
            (h.items ?? []).filter((item) => item.field === "status").map((item) => ({ createdAt: h.created, fromState: { name: item.fromString }, toState: { name: item.toString } })),
          ),
        },
      })),
    },
    started: { nodes: started.map(brief) },
  };
};

// Closing a ticket: the first transition its workflow offers into a Done
// status, then making it. { issue, already, state: { id, name } }, where the
// state's id is the transition's.
const doneStateOf = async (id) => {
  const key = encodeURIComponent(id);
  const [found, moves] = await Promise.all([issue(id), request("GET", `/rest/api/3/issue/${key}/transitions`)]);
  const move = (moves.transitions ?? []).find((t) => t.to?.statusCategory?.key === "done");
  return found && move && { issue: found.identifier, already: found.state.type === "completed", state: { id: move.id, name: move.to?.name ?? move.name } };
};
const closeTicket = async (id, transition) => {
  await request("POST", `/rest/api/3/issue/${encodeURIComponent(id)}/transitions`, { transition: { id: transition } });
  return true;
};

// Where /shape and the other skills file a new ticket: the site, the project
// and the labels from jira.json, never the email or token.
const filing = () => ({ site: site(), team: configValue("jira.json", "jiraProject"), labels: configValue("jira.json", "jiraLabels") ?? [] });

module.exports = {
  key: "jira",
  name: "Jira",
  missing,
  credentials,
  request,
  issue,
  issues,
  detail,
  project,
  assigned,
  search,
  standup,
  doneStateOf,
  closeTicket,
  urlOf,
  filing,
  // For the tests.
  brief,
  jqlTime,
};
