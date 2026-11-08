#!/usr/bin/env node
// Lists the files a diff changes, grouped and ordered for screenshots:
// production source, then config/infra/wiring, then docs, then tests, each
// group in git's order. Prints a plan as JSON for Claude to label and pass to
// render.js. Each file carries its @@ hunk headers and a few changed lines,
// so a label can be written without reading the whole diff.
//
//   node plan.js [--base <ref> | --pr <number>] [--repo <dir>]
//
// With neither, the diff is the current branch against its merge base with
// main (or master, when there is no main).

const { execFileSync } = require("node:child_process");

const args = process.argv.slice(2);
const flag = (name) => {
  const i = args.indexOf(`--${name}`);
  return i >= 0 ? args[i + 1] : undefined;
};
const repo = flag("repo") ?? process.cwd();
const git = (...a) => execFileSync("git", ["-C", repo, ...a], { encoding: "utf8", maxBuffer: 256 * 1024 * 1024 });
const exists = (ref) => {
  try {
    git("rev-parse", "--verify", "--quiet", ref);
    return true;
  } catch {
    return false;
  }
};

let range;
let title;
const pr = flag("pr");
if (pr) {
  // The PR's head and base, fetched by SHA, so nothing is checked out and no
  // branch or ref is added to the repo.
  const info = JSON.parse(execFileSync("gh", ["pr", "view", pr, "--json", "baseRefName,headRefOid,title,number"], { cwd: repo, encoding: "utf8" }));
  git("fetch", "--quiet", "origin", info.baseRefName, `refs/pull/${info.number}/head`);
  range = `origin/${info.baseRefName}...${info.headRefOid}`;
  title = `PR #${info.number}: ${info.title}`;
} else {
  const base = flag("base") ?? (exists("main") ? "main" : exists("master") ? "master" : "origin/HEAD");
  range = `${base}...HEAD`;
  title = `${git("branch", "--show-current").trim() || "HEAD"} against ${base}`;
}

// Lockfiles and generated files are listed as skipped, not rendered: a lockfile's
// diff is thousands of lines no one reads as a picture.
const SKIP = /(^|\/)(package-lock\.json|pnpm-lock\.yaml|yarn\.lock|bun\.lockb?|Gemfile\.lock|poetry\.lock|uv\.lock|Cargo\.lock|composer\.lock|go\.sum)$|\.min\.(js|css)$|\.snap$/;
const TEST = /(^|\/)(__tests__|e2e|tests?|spec)\/|\.(spec|test)\.[^/]+$|_test\.[^/]+$/;
const DOCS = /\.(md|mdx|rst|txt)$|(^|\/)docs?\//i;
const CONFIG = /(^|\/)(\.github|\.circleci|infra|terraform|deploy|k8s|helm|docker|\.husky|\.vscode)\/|(^|\/)(Dockerfile[^/]*|docker-compose[^/]*|Makefile|Procfile|\.env[^/]*|\.gitignore|\.npmrc|\.nvmrc|package\.json|tsconfig[^/]*\.json)$|\.config\.[cm]?[jt]s$|\.(ya?ml|toml|ini|json|tf)$|(^|\/)[^/]*rc(\.[^/]+)?$/;
const groupOf = (path) => (TEST.test(path) ? "tests" : DOCS.test(path) ? "docs" : CONFIG.test(path) ? "config" : "core");
const ORDER = ["core", "config", "docs", "tests"];

// --name-status -M: a rename lists its old and new path, and is diffed as both.
const entries = git("diff", "--name-status", "-M", range)
  .split("\n")
  .filter(Boolean)
  .map((line) => {
    const [status, ...paths] = line.split("\t");
    return { status: status[0], paths, path: paths.at(-1) };
  });

const skipped = entries.filter((e) => SKIP.test(e.path)).map((e) => e.path);
const files = entries
  .filter((e) => !SKIP.test(e.path))
  .map((e, gitOrder) => ({ ...e, gitOrder, group: groupOf(e.path) }))
  .sort((a, b) => ORDER.indexOf(a.group) - ORDER.indexOf(b.group) || a.gitOrder - b.gitOrder)
  .map((e, i) => {
    const diff = git("diff", "-M", range, "--", ...e.paths);
    const lines = diff.split("\n");
    return {
      n: i + 1,
      group: e.group,
      status: e.status,
      paths: e.paths,
      label: "",
      added: lines.filter((l) => l.startsWith("+") && !l.startsWith("+++")).length,
      removed: lines.filter((l) => l.startsWith("-") && !l.startsWith("---")).length,
      hunks: lines.filter((l) => l.startsWith("@@")).slice(0, 8),
      sample: lines.filter((l) => /^[+-](?![+-]{2})/.test(l) && l.trim().length > 2).slice(0, 12).map((l) => l.slice(0, 160)),
    };
  });

process.stdout.write(JSON.stringify({ repo, range, title, files, skipped }, null, 2) + "\n");
