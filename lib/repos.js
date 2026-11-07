// How each repository's work lands: through a pull request ("pr") or straight
// on its default branch ("direct"), as ~/.config/switchboard/repos.json says,
// by the repository's origin on GitHub:
//
//   { "owner/name": { "landing": "direct", "test": "npm test", "squash": true,
//                     "review": "review-loop", "beforePush": "" } }
//
// `landing` is "pr" or "direct"; the rest is how a direct landing goes:
// `test` the command run before the review and again after the last rebase
// (empty, none; not set, what the repository's CLAUDE.md names), `squash` whether the branch lands as one commit, `review`
// "review-loop" or "none", and `beforePush` a command that must pass before
// the push (empty, none). An entry may hold an `apply` list too, which
// bin/apply-actions reads; setRepo keeps it. The file is yours and never in
// this repository.
//
// With no `landing` for a repository, the default decides (landingFor): a pull
// request when GitHub says the default branch needs one (a ruleset, branch
// protection, or a merge queue), else straight on it when the repository's
// CLAUDE.md says its work goes there with no PR, as Haiku reads it, else a
// pull request. The answer is kept a day in the sidebar's state directory
// (repos-landing.json), so nothing waits on GitHub or Claude twice; off the
// network, the last answer stands, and with none it is a pull request, the
// path that can't push to the default branch by mistake.

const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { execFile, execFileSync } = require("node:child_process");
const { countRun } = require("./requests");

const REPOS_FILE = () => process.env.SWITCHBOARD_REPOS || path.join(os.homedir(), ".config/switchboard/repos.json");
const CACHE_FILE = () => path.join(process.env.TMUX_SIDEBAR_STATE_DIR || path.join(os.homedir(), ".local/state/tmux-sidebar"), "repos-landing.json");
const DAY_MS = 24 * 60 * 60 * 1000;

const METHODS = ["pr", "direct"];
const REVIEWS = ["review-loop", "none"];
// A direct landing's steps when its entry doesn't say: `test` null is not
// set, so a landing runs the tests its CLAUDE.md names, where "" is none.
const DIRECT = { test: null, squash: true, review: "review-loop", beforePush: "" };
// What each key takes.
const valid = {
  landing: (v) => METHODS.includes(v),
  test: (v) => typeof v === "string",
  squash: (v) => typeof v === "boolean",
  review: (v) => REVIEWS.includes(v),
  beforePush: (v) => typeof v === "string",
};

// "owner/name" of a git remote's URL on github.com, or undefined.
const repoOfUrl = (url) => /(?:^|[@/])(?:ssh\.)?github\.com(?::\d+)?[:/]([\w.-]+)\/([\w.-]+?)(?:\.git)?$/.exec(String(url).trim())?.slice(1, 3).join("/");

const git = (dir, args) => {
  try {
    return execFileSync("git", ["-C", dir, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 5_000 }).trim();
  } catch {
    return undefined;
  }
};

// The repository at a directory: { repo ("owner/name" on GitHub, else
// undefined), top (the worktree's top), key (what it is known by here) }, or
// undefined outside one.
const repositoryAt = (dir) => {
  const top = git(dir, ["rev-parse", "--show-toplevel"]);
  if (!top) return undefined;
  const repo = repoOfUrl(git(dir, ["remote", "get-url", "origin"]) ?? "");
  const common = git(dir, ["rev-parse", "--path-format=absolute", "--git-common-dir"]);
  return { repo, top, key: repo ?? common ?? top };
};

// The file as it is: {} when it isn't there; throws when it can't be read as
// a JSON object, so nothing writes over it.
const readRepos = () => {
  let text;
  try {
    text = fs.readFileSync(REPOS_FILE(), "utf8");
  } catch (err) {
    if (err.code === "ENOENT") return {};
    throw err;
  }
  const data = JSON.parse(text);
  if (!data || typeof data !== "object" || Array.isArray(data)) throw new Error("it isn't a JSON object");
  return data;
};
const reposSafely = () => {
  try {
    return readRepos();
  } catch {
    return {};
  }
};

// A repository's entry, only the keys that hold a value they can.
const entryOf = (key, data = reposSafely()) => {
  const entry = data[key];
  if (!entry || typeof entry !== "object" || Array.isArray(entry)) return {};
  return Object.fromEntries(Object.entries(entry).filter(([k, v]) => valid[k]?.(v)));
};
const stepsOf = (entry) => Object.fromEntries(Object.keys(DIRECT).map((k) => [k, entry[k] ?? DIRECT[k]]));

// Sets one key of a repository's entry, keeping the rest; `landing`
// "automatic" removes the key, so the default decides, and an entry left
// empty goes. Written to a temporary file and renamed into place, the version
// before kept as repos.json.bak; a file that can't be read is never written
// over.
const setRepo = (key, field, value) => {
  if (!valid[field]) throw new Error(`not a landing setting: ${field}`);
  const automatic = field === "landing" && value === "automatic";
  if (!automatic && !valid[field](value)) throw new Error(`not a value for ${field}: ${JSON.stringify(value)}`);
  const file = REPOS_FILE();
  let data;
  try {
    data = readRepos();
  } catch (err) {
    throw new Error(`${file.replace(os.homedir(), "~")} can't be read (${err.message}); fix it by hand first`);
  }
  const entry = { ...(data[key] && typeof data[key] === "object" ? data[key] : {}) };
  if (automatic) delete entry[field];
  else entry[field] = value;
  if (Object.keys(entry).length) data[key] = entry;
  else delete data[key];
  fs.mkdirSync(path.dirname(file), { recursive: true });
  if (fs.existsSync(file)) fs.copyFileSync(file, `${file}.bak`);
  const tmp = `${file}.${process.pid}.tmp`;
  fs.writeFileSync(tmp, `${JSON.stringify(data, null, 2)}\n`);
  fs.renameSync(tmp, file);
};

const readCache = () => {
  try {
    const data = JSON.parse(fs.readFileSync(CACHE_FILE(), "utf8"));
    return data && typeof data === "object" ? data : {};
  } catch {
    return {};
  }
};
const remember = (key, answer) => {
  try {
    const data = { ...readCache(), [key]: answer };
    fs.mkdirSync(path.dirname(CACHE_FILE()), { recursive: true });
    const tmp = `${CACHE_FILE()}.${process.pid}.tmp`;
    fs.writeFileSync(tmp, JSON.stringify(data));
    fs.renameSync(tmp, CACHE_FILE());
  } catch {}
};
// The answer last found for a repository, however old: { landing, why, at }.
const cachedOf = (key) => {
  const answer = readCache()[key];
  return answer && METHODS.includes(answer.landing) ? answer : undefined;
};

const run = (command, args) =>
  new Promise((resolve, reject) =>
    execFile(command, args, { encoding: "utf8", timeout: 30_000 }, (err, stdout, stderr) => (err ? reject(new Error(String(stderr || err.message).trim().split("\n")[0])) : resolve(stdout))),
  );

// Whether GitHub makes the default branch take a pull request: { branch,
// requiresPr, why }. Throws when GitHub can't be asked.
const askGithub = async (repo) => {
  const [owner, name] = repo.split("/");
  countRun("gh", "GitHub", "GraphQL", "landing: how a repository lands");
  const answer = JSON.parse(
    await run("gh", ["api", "graphql", "-f", "query=query($o: String!, $n: String!) { repository(owner: $o, name: $n) { mergeQueue { url } defaultBranchRef { name refUpdateRule { viewerCanPush requiredApprovingReviewCount } } } }", "-f", `o=${owner}`, "-f", `n=${name}`]),
  );
  const repository = answer?.data?.repository;
  if (!repository) throw new Error(answer?.errors?.[0]?.message ?? "GitHub didn't answer");
  const branch = repository.defaultBranchRef?.name;
  if (repository.mergeQueue) return { branch, requiresPr: true, why: `${branch} has a merge queue` };
  const rule = repository.defaultBranchRef?.refUpdateRule;
  // A count at all, 0 included, is branch protection requiring a pull request
  // (null when it doesn't).
  if (rule && (rule.viewerCanPush === false || typeof rule.requiredApprovingReviewCount === "number")) return { branch, requiresPr: true, why: `GitHub requires a PR on ${branch}` };
  // Rulesets: a repository on a plan without them answers 404, which is none;
  // any other failure is a failure to find out, never read as none.
  let rules = [];
  if (branch) {
    countRun("gh", "GitHub", "REST", "landing: how a repository lands");
    rules = await run("gh", ["api", `repos/${owner}/${name}/rules/branches/${encodeURIComponent(branch)}`]).then(
      (out) => JSON.parse(out),
      (err) => {
        if (/\b404\b|Not Found/.test(err.message)) return [];
        throw err;
      },
    );
  }
  if ([].concat(rules).some((r) => r?.type === "pull_request" || r?.type === "merge_queue")) return { branch, requiresPr: true, why: `GitHub requires a PR on ${branch}` };
  return { branch, requiresPr: false };
};

// Whether a CLAUDE.md says the work goes straight to the default branch with
// no PR, as Haiku reads it, given that file alone.
const askClaudeMd = async (text) => {
  const { askClaude } = require("./sidebar/todo");
  const prompt = `Below is a repository's CLAUDE.md. Does it say that work in this repository is committed straight to its default branch, with no pull request? Answer with the one word "direct" if it does, else "pr".\n\n<claude-md>\n${text}\n</claude-md>`;
  const out = await askClaude(prompt, { model: "haiku", timeout: 120_000, source: "landing: CLAUDE.md", looks: (o) => /\b(direct|pr)\b/i.test(o), what: "direct or pr" });
  return /^\W*direct\b/i.test(out) ? "direct" : "pr";
};

// How the repository at `dir` lands: { repo, key, landing, why, source,
// steps }. `source` is where the method came from: repos.json, github,
// claude-md, default (neither said), cached (a day old at most), or offline
// (GitHub or Claude couldn't be asked: the last answer, else a pull request).
// `ask` replaces GitHub and Claude in the tests.
const landingFor = async (dir, { ask = {}, now = Date.now(), refresh = false } = {}) => {
  const at = repositoryAt(dir);
  if (!at) throw new Error(`${dir} isn't in a git repository`);
  // A repos.json that can't be read stops it, rather than landing without
  // what it may say.
  let data;
  try {
    data = readRepos();
  } catch (err) {
    throw new Error(`${REPOS_FILE().replace(os.homedir(), "~")} can't be read (${err.message}); fix it first`);
  }
  const entry = entryOf(at.key, data);
  const base = { repo: at.repo, key: at.key, steps: stepsOf(entry) };
  if (entry.landing) return { ...base, landing: entry.landing, why: "set in repos.json", source: "repos.json" };
  const cached = cachedOf(at.key);
  if (cached && !refresh && now - cached.at < DAY_MS) return { ...base, landing: cached.landing, why: cached.why, source: "cached" };
  // The github setting off asks GitHub nothing: the last answer stands,
  // else a pull request.
  if (require("./settings").setting("github") === false) {
    if (cached) return { ...base, landing: cached.landing, why: `${cached.why} (as last found; the github setting is off)`, source: "offline" };
    return { ...base, landing: "pr", why: "the github setting is off, so through a PR unless repos.json says otherwise", source: "offline" };
  }
  const github = ask.github ?? askGithub;
  const claude = ask.claude ?? askClaudeMd;
  try {
    const gh = at.repo ? await github(at.repo) : { requiresPr: false };
    let answer;
    if (gh.requiresPr) answer = { landing: "pr", why: gh.why, source: "github" };
    else {
      let text;
      try {
        // Only a file inside the repository, a link out of it never followed,
        // since what is read is sent to Claude.
        const file = fs.realpathSync(path.join(at.top, "CLAUDE.md"));
        if (file.startsWith(`${fs.realpathSync(at.top)}${path.sep}`)) text = fs.readFileSync(file, "utf8");
      } catch {}
      const said = text?.trim() ? await claude(text) : undefined;
      answer = said === "direct" ? { landing: "direct", why: `its CLAUDE.md commits straight to ${gh.branch ?? "the default branch"}`, source: "claude-md" } : { landing: "pr", why: text?.trim() ? "its CLAUDE.md doesn't say to commit straight to the default branch" : "nothing says otherwise", source: "default" };
    }
    remember(at.key, { landing: answer.landing, why: answer.why, at: now });
    return { ...base, ...answer };
  } catch (err) {
    if (cached) return { ...base, landing: cached.landing, why: `${cached.why} (as last found; couldn't ask again: ${err.message})`, source: "offline" };
    return { ...base, landing: "pr", why: `couldn't find out (${err.message}), so through a PR`, source: "offline" };
  }
};

module.exports = { REPOS_FILE, CACHE_FILE, DAY_MS, METHODS, REVIEWS, DIRECT, repoOfUrl, repositoryAt, readRepos, reposSafely, entryOf, stepsOf, setRepo, cachedOf, landingFor, askGithub, askClaudeMd };
