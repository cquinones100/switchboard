// Tests for lib/tracker: Jira's rich text as Markdown, ticket IDs from links,
// the tracker setting (and Linear's old on/off one), and the Jira provider and
// the shell scripts' CLI against a stand-in for Jira's API on localhost. Run
// with: node --test test/*.test.js
//
// Everything runs with a temporary HOME and config, and Jira's token, email
// and site come from the environment, so nothing reads your Keychain or
// settings, or reaches Jira or Linear.

const { test, before, after } = require("node:test");
const assert = require("node:assert/strict");
const http = require("node:http");
const fs = require("node:fs");
const path = require("node:path");
const { execFile, execFileSync } = require("node:child_process");
const { promisify } = require("node:util");

const tmp = fs.realpathSync(fs.mkdtempSync("/tmp/tracker-"));
const settingsFile = path.join(tmp, "settings.json");
process.env.HOME = tmp;
process.env.SWITCHBOARD_SETTINGS = settingsFile;
process.env.JIRA_EMAIL = "me@example.com";
process.env.JIRA_API_TOKEN = "token";

const repo = path.join(__dirname, "..");
const { markdownOf } = require("../lib/tracker/adf");
const { idFrom, tracker } = require("../lib/tracker");
const jira = require("../lib/tracker/jira");
const settings = require("../lib/settings");

// The stand-in: answers by method and path, and keeps every request.
const requests = [];
let routes = {};
const server = http.createServer((req, res) => {
  let body = "";
  req.on("data", (chunk) => (body += chunk));
  req.on("end", () => {
    const url = new URL(req.url, "http://localhost");
    requests.push({ method: req.method, path: url.pathname, query: url.searchParams, auth: req.headers.authorization, body: body && JSON.parse(body) });
    const route = routes[`${req.method} ${url.pathname}`];
    const [status, answer] = route ? route(requests.at(-1)) : [404, { errorMessages: ["Issue does not exist or you do not have permission to see it."] }];
    res.writeHead(status, { "Content-Type": "application/json" });
    res.end(answer === undefined ? "" : JSON.stringify(answer));
  });
});

before(async () => {
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  process.env.JIRA_SITE = `http://127.0.0.1:${server.address().port}`;
});
after(() => {
  server.close();
  fs.rmSync(tmp, { recursive: true, force: true });
});

const status = (name, key) => ({ name, statusCategory: { key } });
const epic = { key: "AB-1", fields: { summary: "The epic", issuetype: { name: "Epic", hierarchyLevel: 1 } } };
const issue = (key, fields = {}) => ({ key, fields: { summary: `Work on ${key}`, status: status("In Progress", "indeterminate"), priority: { name: "High" }, parent: epic, ...fields } });
const doc = (...content) => ({ type: "doc", version: 1, content });
const para = (...content) => ({ type: "paragraph", content });
const text = (t, marks) => ({ type: "text", text: t, ...(marks ? { marks } : {}) });

test("Jira's rich text reads as Markdown: headings, marks, links, lists, code, tasks, tables and panels", () => {
  const md = markdownOf(
    doc(
      { type: "heading", attrs: { level: 2 }, content: [text("Why")] },
      para(text("Make "), text("this", [{ type: "strong" }]), text(" work, as "), text("the doc", [{ type: "link", attrs: { href: "https://example.com" } }]), text(" says "), { type: "mention", attrs: { text: "@Ada" } }),
      {
        type: "bulletList",
        content: [
          { type: "listItem", content: [para(text("one")), { type: "bulletList", content: [{ type: "listItem", content: [para(text("nested"))] }] }] },
          { type: "listItem", content: [para(text("two"))] },
        ],
      },
      { type: "orderedList", attrs: { order: 3 }, content: [{ type: "listItem", content: [para(text("third"))] }] },
      { type: "codeBlock", attrs: { language: "mermaid" }, content: [text("graph TD\nA-->B")] },
      { type: "taskList", content: [{ type: "taskItem", attrs: { state: "DONE" }, content: [text("done")] }, { type: "taskItem", attrs: { state: "TODO" }, content: [text("to do")] }] },
      {
        type: "table",
        content: [
          { type: "tableRow", content: [{ type: "tableHeader", content: [para(text("A"))] }, { type: "tableHeader", content: [para(text("B"))] }] },
          { type: "tableRow", content: [{ type: "tableCell", content: [para(text("1"))] }, { type: "tableCell", content: [para(text("2|3"))] }] },
        ],
      },
      { type: "panel", attrs: { panelType: "warning" }, content: [para(text("careful"))] },
      { type: "mediaSingle", content: [{ type: "media", attrs: { alt: "shot.png" } }] },
      { type: "someNodeFromTheFuture", content: [para(text("still read"))] },
    ),
  );
  assert.equal(
    md,
    [
      "## Why",
      "Make **this** work, as [the doc](https://example.com) says @Ada",
      "- one\n  - nested\n- two",
      "3. third",
      "```mermaid\ngraph TD\nA-->B\n```",
      "- [x] done\n- [ ] to do",
      "| A | B |\n| --- | --- |\n| 1 | 2\\|3 |",
      "> **Warning:** careful",
      "_[attachment: shot.png]_",
      "still read",
    ].join("\n\n"),
  );
  assert.equal(markdownOf(null), "");
  assert.equal(markdownOf("plain text, from an older API"), "plain text, from an older API");
});

test("a ticket ID comes from an ID, a Linear link or a Jira one, upper-cased", () => {
  assert.deepEqual(
    ["ab-12", "https://linear.app/x/issue/ENG-4/some-title", "https://x.atlassian.net/browse/PLAT-9?focused=1", "https://x.atlassian.net/jira/software/projects/P/boards/1?selectedIssue=P-3", "fix-it-later", "nope"].map(idFrom),
    ["AB-12", "ENG-4", "PLAT-9", "P-3", undefined, undefined],
  );
});

test("the tracker setting picks Linear, Jira or none, and Linear turned off before it existed is the tracker off", () => {
  const pick = (saved) => {
    if (saved) fs.writeFileSync(settingsFile, JSON.stringify(saved));
    else fs.rmSync(settingsFile, { force: true });
    settings.reloadSettings();
    return tracker()?.key;
  };
  try {
    assert.equal(pick(undefined), "linear", "Linear by default");
    assert.equal(pick({ tracker: "jira" }), "jira");
    assert.equal(pick({ tracker: "off" }), undefined);
    assert.equal(pick({ linear: false }), undefined, "Linear's old off");
    assert.equal(settings.isOn("tracker"), false);
    assert.equal(pick({ linear: false, tracker: "jira" }), "jira", "the new setting wins");

    // Changing it leaves the old key behind.
    fs.writeFileSync(settingsFile, JSON.stringify({ linear: false, tips: false }));
    settings.writeSetting("tracker", "jira");
    assert.deepEqual(JSON.parse(fs.readFileSync(settingsFile, "utf8")), { tips: false, tracker: "jira" });
  } finally {
    fs.rmSync(settingsFile, { force: true });
    settings.reloadSettings();
  }
});

test("settings.sh reads Linear's old off as the tracker off, as lib/settings.js does", () => {
  const socket = path.join(tmp, "tmux.sock");
  const env = { ...process.env, TMUX: "", SWITCHBOARD_DIR: repo };
  const tmux = (...args) => execFileSync("tmux", ["-S", socket, ...args], { encoding: "utf8", env }).trim();
  try {
    tmux("-f", "/dev/null", "new-session", "-d", "-s", "t", "sleep 60");
    fs.writeFileSync(settingsFile, JSON.stringify({ linear: false }));
    execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: { ...env, TMUX: `${socket},0,0` } });
    assert.equal(tmux("show", "-gqv", "@setting_tracker"), "off");
    fs.writeFileSync(settingsFile, JSON.stringify({ tracker: "jira" }));
    execFileSync("bash", [path.join(repo, "tmux/settings.sh"), "apply", "--load"], { env: { ...env, TMUX: `${socket},0,0` } });
    assert.equal(tmux("show", "-gqv", "@setting_tracker"), "jira");
  } finally {
    execFile("tmux", ["-S", socket, "kill-server"], () => {});
    fs.rmSync(settingsFile, { force: true });
  }
});

test("a refused Jira token cut off at 128 characters, as the Keychain's hidden prompt stores one, says so", async () => {
  routes = { "GET /rest/api/3/issue/AB-2": () => [401, {}] };
  process.env.JIRA_API_TOKEN = `ATATT3x${"a".repeat(121)}`;
  try {
    await assert.rejects(jira.issue("AB-2"), /cut off at 128 characters/);
  } finally {
    process.env.JIRA_API_TOKEN = "token";
  }
});

test("a Jira that answers not found because it refused the token says so, rather than that there is no such issue", async () => {
  routes = { "GET /rest/api/3/myself": () => [401, {}] };
  await assert.rejects(jira.issue("AB-2"), /may have expired/);
});

test("Jira's issue comes back in Linear's shape: its status category as the state's type, its epic as the project", async () => {
  routes = {
    "GET /rest/api/3/myself": () => [200, { displayName: "Me" }],
    "GET /rest/api/3/issue/AB-2": () => [200, issue("AB-2")],
    "GET /rest/api/3/issue/AB-3": () => [200, issue("AB-3", { status: status("Won't Do", "done"), resolution: { name: "Won't Do" }, parent: undefined })],
    "GET /rest/api/3/issue/AB-4": () => [200, issue("AB-4", { status: status("CANCELLED", "done"), parent: undefined })],
  };
  requests.length = 0;
  assert.deepEqual(await jira.issue("AB-2"), {
    identifier: "AB-2",
    title: "Work on AB-2",
    url: `${process.env.JIRA_SITE}/browse/AB-2`,
    branchName: null,
    state: { name: "In Progress", type: "started" },
    project: { name: "The epic", url: `${process.env.JIRA_SITE}/browse/AB-1` },
    priority: 2,
    priorityLabel: "High",
    updatedAt: undefined,
    completedAt: null,
  });
  assert.equal(requests[0].auth, `Basic ${Buffer.from("me@example.com:token").toString("base64")}`);
  assert.deepEqual((await jira.issue("AB-3")).state, { name: "Won't Do", type: "canceled" });
  assert.deepEqual((await jira.issue("AB-4")).state, { name: "CANCELLED", type: "canceled" }, "by its name, with no resolution");
  assert.equal(await jira.issue("AB-404"), null, "no such issue is null, not an error");
});

test("a refused Jira token says it may have expired", async () => {
  routes = { "GET /rest/api/3/issue/AB-2": () => [401, { errorMessages: ["Unauthorized"] }] };
  await assert.rejects(jira.issue("AB-2"), /may have expired/);
});

test("Jira's ticket detail is the ticket view's shape, its description and comments as Markdown", async () => {
  routes = {
    "GET /rest/api/3/issue/AB-2": () => [
      200,
      issue("AB-2", {
        description: doc(para(text("Do "), text("it", [{ type: "em" }]))),
        assignee: { displayName: "Ada" },
        reporter: { displayName: "Lin" },
        created: "2026-10-01T10:00:00.000+0000",
        updated: "2026-10-02T10:00:00.000+0000",
        labels: ["backend"],
        subtasks: [{ key: "AB-5", fields: { summary: "A part", status: { name: "To Do" } } }],
        issuelinks: [{ type: { name: "Blocks", outward: "blocks", inward: "is blocked by" }, inwardIssue: { key: "AB-6", fields: { summary: "Before", status: { name: "Done" } } } }],
      }),
    ],
    "GET /rest/api/3/issue/AB-2/comment": () => [200, { comments: [{ id: "10", author: { displayName: "Ada" }, created: "2026-10-01T11:00:00.000+0000", body: doc(para(text("Looks good"))) }] }],
    "GET /rest/api/3/issue/AB-2/remotelink": () => [200, [{ object: { url: "https://example.com/spec", title: "Spec" } }]],
  };
  const found = await jira.detail("AB-2");
  assert.equal(found.tracker, "Jira");
  assert.equal(found.description, "Do _it_");
  assert.equal(found.assignee.name, "Ada");
  assert.equal(found.creator.name, "Lin");
  assert.deepEqual(found.labels.nodes, [{ name: "backend" }]);
  assert.deepEqual(found.children.nodes, [{ identifier: "AB-5", title: "A part", state: { name: "To Do" } }]);
  assert.deepEqual(found.relations.nodes, [{ type: "Is blocked by", relatedIssue: { identifier: "AB-6", title: "Before", state: { name: "Done" } } }]);
  assert.deepEqual(found.attachments.nodes, [{ title: "Spec", url: "https://example.com/spec" }]);
  assert.deepEqual(found.comments.nodes, [{ id: "10", body: "Looks good", createdAt: "2026-10-01T11:00:00.000+0000", parent: null, user: { name: "Ada" }, botActor: null }]);
  assert.equal(found.parent, null, "an epic is the project, not the parent");
  assert.equal(await jira.detail("AB-404"), null);
});

test("an epic's sub-issues in the ticket view are the issues under it", async () => {
  routes = {
    "GET /rest/api/3/issue/AB-1": () => [200, { key: "AB-1", fields: { summary: "The epic", status: status("Backlog", "new"), issuetype: { name: "Epic", hierarchyLevel: 1 } } }],
    "GET /rest/api/3/issue/AB-1/comment": () => [200, { comments: [] }],
    "GET /rest/api/3/issue/AB-1/remotelink": () => [200, []],
    "POST /rest/api/3/search/jql": (req) => [200, { issues: req.body.jql.startsWith("parent = AB-1") ? [issue("AB-2"), issue("AB-3")] : [], isLast: true }],
  };
  assert.deepEqual((await jira.detail("AB-1")).children.nodes.map((c) => c.identifier), ["AB-2", "AB-3"]);
});

test("Jira's searches go through JQL, every page, and its text search keeps only words", async () => {
  const pages = [
    { issues: [issue("AB-7")], isLast: false, nextPageToken: "two" },
    { issues: [issue("AB-8")], isLast: true },
  ];
  routes = {
    "POST /rest/api/3/search/jql": (req) => [200, req.body.nextPageToken ? pages[1] : pages[0]],
    "GET /rest/api/3/issue/AB-8": () => [200, issue("AB-8")],
  };
  requests.length = 0;
  assert.deepEqual((await jira.assigned("2026-10-22T00:00:00Z")).map((i) => i.identifier), ["AB-7", "AB-8"]);
  assert.match(requests[0].body.jql, /^assignee = currentUser\(\) AND \(statusCategory != Done OR updated >= "2026-10-2\d \d\d:\d\d"\) ORDER BY updated DESC$/);
  assert.equal(requests[1].body.nextPageToken, "two");

  requests.length = 0;
  assert.deepEqual((await jira.search('fix "login" (redirect)')).map((i) => i.identifier), ["AB-7", "AB-8"]);
  assert.equal(requests[0].body.jql, 'text ~ "fix login redirect" ORDER BY updated DESC');

  requests.length = 0;
  assert.deepEqual((await jira.search("ab-8")).map((i) => i.identifier), ["AB-8", "AB-7"], "a key searched for comes first, once");
});

test("the standup's ticket moves come from Jira's changelog", async () => {
  routes = {
    "POST /rest/api/3/search/jql": (req) => [
      200,
      req.body.expand === "changelog"
        ? { issues: [{ ...issue("AB-9"), changelog: { histories: [{ created: "2026-10-23T09:00:00.000+0000", items: [{ field: "status", fromString: "To Do", toString: "In Progress" }, { field: "assignee", fromString: null, toString: "Ada" }] }] } }], isLast: true }
        : { issues: [issue("AB-10")], isLast: true },
    ],
  };
  const data = await jira.standup("2026-10-23T00:00:00Z");
  assert.deepEqual(data.changed.nodes[0].history.nodes, [{ createdAt: "2026-10-23T09:00:00.000+0000", fromState: { name: "To Do" }, toState: { name: "In Progress" } }]);
  assert.deepEqual(data.started.nodes.map((i) => [i.identifier, i.state.name]), [["AB-10", "In Progress"]]);
});

test("closing a Jira ticket takes the first transition into Done", async () => {
  routes = {
    "GET /rest/api/3/issue/AB-2": () => [200, issue("AB-2")],
    "GET /rest/api/3/issue/AB-2/transitions": () => [200, { transitions: [{ id: "11", name: "Start", to: status("In Progress", "indeterminate") }, { id: "31", name: "Finish", to: status("Shipped", "done") }] }],
    "POST /rest/api/3/issue/AB-2/transitions": () => [204],
  };
  const done = await jira.doneStateOf("AB-2");
  assert.deepEqual(done, { issue: "AB-2", already: false, state: { id: "31", name: "Shipped" } });
  requests.length = 0;
  assert.equal(await jira.closeTicket(done.issue, done.state.id), true);
  assert.deepEqual(requests[0].body, { transition: { id: "31" } });
});

test("the shell scripts' CLI answers from Jira, with exit codes for no such ticket and no epic", async () => {
  fs.writeFileSync(settingsFile, JSON.stringify({ tracker: "jira" }));
  routes = {
    "GET /rest/api/3/myself": () => [200, { displayName: "Me" }],
    "GET /rest/api/3/issue/AB-2": () => [200, issue("AB-2")],
    "GET /rest/api/3/issue/AB-3": () => [200, issue("AB-3", { parent: undefined })],
  };
  const cli = (...args) =>
    promisify(execFile)("node", [path.join(repo, "lib/tracker/cli.js"), ...args], { encoding: "utf8" }).then(
      (r) => ({ code: 0, out: r.stdout.trim() }),
      (err) => ({ code: err.code, out: err.stdout.trim(), err: err.stderr.trim() }),
    );
  try {
    assert.deepEqual(await cli("name"), { code: 0, out: "Jira" });
    assert.deepEqual(await cli("title", "ab-2"), { code: 0, out: "Work on AB-2" });
    assert.equal((await cli("title", "AB-404")).code, 3);
    assert.deepEqual(await cli("url", "https://x.atlassian.net/browse/AB-2"), { code: 0, out: `${process.env.JIRA_SITE}/browse/AB-2` });
    assert.deepEqual(await cli("project", "AB-2"), { code: 0, out: `${process.env.JIRA_SITE}/browse/AB-1` });
    assert.equal((await cli("project", "AB-3")).code, 4);
    // The site from ~/.config/switchboard/jira.json, without JIRA_SITE.
    const own = path.join(tmp, ".config/switchboard");
    fs.mkdirSync(own, { recursive: true });
    fs.writeFileSync(path.join(own, "jira.json"), JSON.stringify({ jiraSite: "acme.atlassian.net", jiraEmail: "me@acme.com", jiraApiToken: "secret", jiraProject: "AB", jiraLabels: ["web"] }));
    const site = process.env.JIRA_SITE;
    delete process.env.JIRA_SITE;
    try {
      assert.deepEqual(await cli("url", "AB-2"), { code: 0, out: "https://acme.atlassian.net/browse/AB-2" });
      // Where tickets are filed, and nothing secret.
      assert.deepEqual(JSON.parse((await cli("config")).out), { tracker: "Jira", site: "https://acme.atlassian.net", team: "AB", labels: ["web"] });
    } finally {
      process.env.JIRA_SITE = site;
      fs.rmSync(path.join(own, "jira.json"));
    }
    // Linear's, from linear.json, without its key.
    fs.writeFileSync(settingsFile, JSON.stringify({ tracker: "linear" }));
    fs.writeFileSync(path.join(own, "linear.json"), JSON.stringify({ linearWorkspace: "acme", linearTeam: "AB", linearApiKey: "secret" }));
    try {
      assert.deepEqual(JSON.parse((await cli("config")).out), { tracker: "Linear", site: "https://linear.app/acme", team: "AB", labels: [] });
    } finally {
      fs.rmSync(path.join(own, "linear.json"));
    }
    fs.writeFileSync(settingsFile, JSON.stringify({ tracker: "off" }));
    assert.equal((await cli("title", "AB-2")).code, 2);
    assert.equal((await cli("config")).code, 2);
  } finally {
    fs.rmSync(settingsFile, { force: true });
  }
});
