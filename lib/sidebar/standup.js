// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// The standup draft: each weekday morning, once the daemon runs after
// STANDUP_HOUR, a note in the notes app, Standups/Standup <date>.md, written
// from the last working day: the pull requests you merged, opened or worked
// on and the ones you reviewed (one GitHub search through gh), the tickets
// assigned to you or made by you that changed state (from Linear or Jira, as
// the tracker setting says, through lib/tracker; a ticket closed by a merge
// often has no assignee), and the
// commits you made in the repositories your tabs are in (git, each main
// clone's branches, a squash merge left to its PR); then what is in progress
// today and the reviews asked of you in the last WAITING_DAYS, and an empty
// Blockers list for you to fill. A note already there is
// never written over, so it is yours to edit once written. Off the drawing
// path, retried every STANDUP_RETRY_MS while it fails. Only while the
// standup_note setting is on; GitHub's part only with github on, the tickets
// only with the tracker on.

const fs = require("node:fs");
const { noteWritten } = require("./new-notes");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { tracker } = require("../tracker");
const { repoOf } = require("./prs");

const STANDUP_HOUR = 6;
const STANDUP_RETRY_MS = 15 * 60_000;
// Reviews asked of you that nothing has touched for longer are left out.
const WAITING_DAYS = 14;
const NOTES = () => process.env.NOTES_DIR || path.join(os.homedir(), "Notes");

const run = (cmd, args, opts = {}) =>
  new Promise((resolve) => {
    execFile(cmd, args, { encoding: "utf8", timeout: 30_000, maxBuffer: 16 * 1024 * 1024, ...opts }, (err, stdout) => resolve(err ? undefined : stdout));
  });

const ymd = (d) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const startOfDay = (d) => new Date(d.getFullYear(), d.getMonth(), d.getDate());

// The working day before today's: Friday on a Monday.
const lastWorkday = (now) => {
  const day = startOfDay(now);
  do day.setDate(day.getDate() - 1);
  while ([0, 6].includes(day.getDay()));
  return day;
};

const standupFile = (now) => path.join(NOTES(), "Standups", `Standup ${ymd(now)}.md`);

// Whether the daemon should write today's note now.
const standupDue = (now, triedAt = 0) =>
  ![0, 6].includes(now.getDay()) && now.getHours() >= STANDUP_HOUR && now - triedAt >= STANDUP_RETRY_MS && !fs.existsSync(standupFile(now));

const GITHUB = `query($mine: String!, $reviewed: String!, $waiting: String!) { viewer { login }
  mine: search(query: $mine, type: ISSUE, first: 50) { nodes { ... on PullRequest { number title url state isDraft createdAt mergedAt repository { nameWithOwner } } } }
  reviewed: search(query: $reviewed, type: ISSUE, first: 50) { nodes { ... on PullRequest { number title url repository { nameWithOwner }
    reviews(last: 30) { nodes { author { login } submittedAt } } } } }
  waiting: search(query: $waiting, type: ISSUE, first: 20) { nodes { ... on PullRequest { number title url repository { nameWithOwner } } } } }`;

// The note's Markdown from what was fetched, each part left out when it has
// nothing: { github: the GitHub answer's data, tickets: the tracker's
// (lib/tracker's standup, in Linear's shape), commits: [{ repo, subjects }] }.
const standupFrom = ({ github, tickets, commits = [] }, now) => {
  const from = lastWorkday(now);
  const to = startOfDay(now);
  const within = (iso) => iso && Date.parse(iso) >= from && Date.parse(iso) < to;
  const pr = (p) => `[${p.repository.nameWithOwner.split("/")[1]}#${p.number} ${p.title}](${p.url})`;
  const ticket = (t) => `[${t.identifier} ${t.title}](${t.url})`;
  const nodes = (search) => (search?.nodes ?? []).filter((n) => n?.number);

  const mine = nodes(github?.mine);
  const prLines = [
    ...mine.filter((p) => within(p.mergedAt)).map((p) => `Merged ${pr(p)}`),
    ...mine.filter((p) => !within(p.mergedAt) && within(p.createdAt)).map((p) => `Opened ${pr(p)}${p.isDraft ? " (draft)" : ""}`),
    ...mine.filter((p) => !within(p.mergedAt) && !within(p.createdAt) && p.state === "OPEN").map((p) => `Worked on ${pr(p)}`),
    ...nodes(github?.reviewed)
      .filter((p) => (p.reviews?.nodes ?? []).some((r) => r.author?.login === github.viewer?.login && within(r.submittedAt)))
      .map((p) => `Reviewed ${pr(p)}`),
  ];
  // A ticket's moves that day, from where it started to where it ended.
  const ticketLines = (tickets?.changed?.nodes ?? []).flatMap((t) => {
    const moves = (t.history?.nodes ?? []).filter((h) => h.toState && within(h.createdAt)).sort((a, b) => Date.parse(a.createdAt) - Date.parse(b.createdAt));
    return moves.length ? [`${ticket(t)}: ${moves[0].fromState?.name ?? "new"} → ${moves.at(-1).toState.name}`] : [];
  });
  const commitLines = commits
    .filter((c) => c.subjects.length)
    .map((c) => `Commits in ${c.repo}: ${c.subjects.slice(0, 5).join("; ")}${c.subjects.length > 5 ? `; and ${c.subjects.length - 5} more` : ""}`);
  const todayLines = [
    ...(tickets?.started?.nodes ?? []).map((t) => `${ticket(t)} (${t.state.name})`),
    ...nodes(github?.waiting).map((p) => `Review ${pr(p)}`),
  ];
  const list = (lines) => (lines.length ? lines.map((l) => `- ${l}`).join("\n") : "- Nothing found");
  const day = from.toLocaleDateString("en-US", { weekday: "long", month: "short", day: "numeric" });
  return `# Standup ${ymd(now)}\n\n## ${day}\n\n${list([...prLines, ...ticketLines, ...commitLines])}\n\n## Today\n\n${list(todayLines)}\n\n## Blockers\n\n- \n`;
};

// Your commits on the last working day in each repository the tabs are in,
// newest first, a subject once however many branches carry it.
const commitsOf = async (tabs, now) => {
  const from = lastWorkday(now);
  const dirs = [...new Set(tabs.map((t) => t.path).filter(Boolean))];
  const clones = new Map();
  await Promise.all(
    dirs.map(async (dir) => {
      const common = (await run("git", ["-C", dir, "rev-parse", "--path-format=absolute", "--git-common-dir"]))?.trim();
      const repo = repoOf(dir);
      if (common && repo && !clones.has(repo)) clones.set(repo, path.dirname(common));
    }),
  );
  return Promise.all(
    [...clones].map(async ([repo, dir]) => {
      const email = (await run("git", ["-C", dir, "config", "user.email"]))?.trim();
      if (!email) return { repo, subjects: [] };
      const out = await run("git", ["-C", dir, "log", "--all", "--no-merges", `--author=${email}`, `--since=${from.toISOString()}`, `--until=${startOfDay(now).toISOString()}`, "--format=%s"]);
      // A squash merge's commit ends in its PR's number, and the PR is listed already.
      return { repo, subjects: [...new Set((out ?? "").split("\n").filter((l) => l && !/\(#\d+\)$/.test(l)))] };
    }),
  );
};

// Fetches and writes today's note; resolves to the file, or undefined when a
// source that is on couldn't be read, to be tried again later.
const writeStandup = async (tabs, { github = true, tickets = true } = {}, now = new Date(), log = () => {}) => {
  const since = lastWorkday(now);
  let githubData;
  if (github) {
    const day = ymd(since);
    const out = await run("gh", ["api", "graphql", "-f", `query=${GITHUB}`, "-f", `mine=is:pr author:@me archived:false updated:>=${day}`, "-f", `reviewed=is:pr reviewed-by:@me -author:@me updated:>=${day}`,
      "-f", `waiting=is:pr is:open archived:false review-requested:@me updated:>=${ymd(new Date(now - WAITING_DAYS * 86_400_000))}`]);
    githubData = out && JSON.parse(out).data;
    if (!githubData) return log("standup: GitHub couldn't be read; trying again later");
  }
  let ticketData;
  const provider = tickets && tracker();
  if (provider && (await provider.credentials())) {
    try {
      ticketData = await provider.standup(since.toISOString());
    } catch (err) {
      return log(`standup: ${provider.name} couldn't be read (${String(err.message).split("\n")[0]}); trying again later`);
    }
  }
  const file = standupFile(now);
  fs.mkdirSync(path.dirname(file), { recursive: true });
  try {
    fs.writeFileSync(file, standupFrom({ github: githubData, tickets: ticketData, commits: await commitsOf(tabs, now) }, now), { flag: "wx" });
    noteWritten(file, "the standup");
  } catch (err) {
    if (err.code !== "EEXIST") throw err;
  }
  log(`standup: wrote ${file}`);
  return file;
};

module.exports = { STANDUP_HOUR, lastWorkday, standupFile, standupDue, standupFrom, writeStandup };
