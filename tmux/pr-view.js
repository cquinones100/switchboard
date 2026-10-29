#!/usr/bin/env node

// For tmux/pr-view.sh and tmux/ticket-view.sh: turns a PR (gh's JSON) or a
// ticket (Linear's GraphQL answer, or Jira's in its shape, from lib/tracker)
// into the Markdown glow draws, with each Mermaid diagram drawn as text by
// mermaid-ascii, and puts the drawings back into glow's output untouched.
//
//   pr-view.js prepare <pr.json> <dir> <columns>       writes <dir>/pr.md
//   pr-view.js ticket <ticket.json> <dir> <columns>    writes <dir>/pr.md
//   pr-view.js page <page.md> <dir> <columns>          writes <dir>/pr.md, a web page link-view.sh read
//   pr-view.js splice <dir> < glow's output            prints it with the drawings in
//   pr-view.js top <pr.txt> <line>                     the line of <pr.txt> that shows <line>
//
// glow prints a link's URL after its text, which in a PR or ticket is mostly
// long and says nothing the text doesn't. So a link goes to glow as its text
// alone, between markers, and splice makes that text the link again (an OSC 8
// hyperlink, in glow's link colour). A bare URL, which has no text, stays as
// glow prints it.
//
// glow wraps every line to the pane, a code block's too, which cuts a
// drawing's boxes apart, so a drawing goes in as a placeholder line and comes
// back after. A left-to-right flowchart wider than the pane is drawn top to
// bottom instead, which is as wide as its widest box; one still wider is left
// whole, for the pager to cut at the pane's edge (less -S) rather than wrap.
// A diagram mermaid-ascii can't draw (it does flowcharts and sequence
// diagrams) keeps its source, said so.

const { execFileSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const MARK = "PRVIEWDIAGRAM";
const widest = (text) => Math.max(0, ...text.split("\n").map((line) => [...line].length));

const draw = (source, dir, n, columns) => {
  const file = path.join(dir, `${n}.mmd`);
  const run = (text) => {
    fs.writeFileSync(file, text);
    try {
      return execFileSync("mermaid-ascii", ["-f", file, "--max-width", String(columns), "-x", "2", "-y", "1"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).replace(/\s+$/, "");
    } catch {
      return undefined;
    }
  };
  let drawn = run(source);
  const sideways = /^\s*(flowchart|graph)\s+(LR|RL)\b/m;
  if (drawn !== undefined && widest(drawn) > columns && sideways.test(source)) {
    drawn = run(source.replace(sideways, "$1 TD")) ?? drawn;
  }
  return drawn;
};

// Each Mermaid block in <text> drawn and swapped for its placeholder; <where>
// says where a diagram that can't be drawn is shown properly.
let drawings = 0;
const diagrams = (text, dir, columns, where) =>
  (text ?? "").replace(/\r/g, "").replace(/^[ \t]*```+[ \t]*mermaid[ \t]*\n([\s\S]*?)^[ \t]*```+[ \t]*$/gm, (whole, source) => {
    const n = ++drawings;
    const drawn = draw(source, dir, n, columns);
    if (drawn === undefined || drawn === "") return `_A Mermaid diagram mermaid-ascii cannot draw; ${where} shows it:_\n\n\`\`\`mermaid\n${source}\`\`\``;
    fs.writeFileSync(path.join(dir, `${n}.txt`), drawn);
    return `${MARK}${n}`;
  });

const prepare = (prFile, dir, columns) => {
  const pr = JSON.parse(fs.readFileSync(prFile, "utf8"));
  const header = `# ${pr.title} #${pr.number}\n\n_${pr.isDraft ? "Draft" : pr.state} · ${pr.author?.login ?? "someone"} wants to merge \`${pr.headRefName}\` into \`${pr.baseRefName}\`_\n\n---\n\n`;
  fs.writeFileSync(path.join(dir, "pr.md"), linkify(header + diagrams(pr.body, dir, columns, "M-r on GitHub"), dir));
};

// A web page link-view.sh read (lib/link-view.js), its diagrams and links as a
// PR's.
const webPage = (mdFile, dir, columns) => {
  fs.writeFileSync(path.join(dir, "pr.md"), linkify(diagrams(fs.readFileSync(mdFile, "utf8"), dir, columns, "the page in the browser"), dir));
};

// A ticket as Linear's page lays it out: title, the facts in its side panel,
// the description, sub-issues and related tickets, then the comments, oldest
// first, each reply under the comment it answers.
const day = (iso) => (iso ?? "").slice(0, 10);
// A bare Linear or Jira ticket URL as a link named by its ID, which then
// reads as the ID, clickable.
const tidy = (text) =>
  (text ?? "")
    .replace(/(^|[^(<])<?(https:\/\/linear\.app\/[^/\s]+\/issue\/([A-Z][A-Z0-9]*-\d+)[^\s)>]*)>?/g, "$1[$3]($2)")
    .replace(/(^|[^(<])<?(https:\/\/[\w-]+\.atlassian\.net\/browse\/([A-Z][A-Z0-9]*-\d+)[^\s)>]*)>?/g, "$1[$3]($2)");
const ticket = (ticketFile, dir, columns) => {
  const issue = JSON.parse(fs.readFileSync(ticketFile, "utf8")).data.issue;
  const facts = [
    issue.state?.name,
    issue.priorityLabel !== "No priority" && issue.priorityLabel,
    issue.assignee ? `assigned to ${issue.assignee.name}` : "unassigned",
    issue.project?.name && `project ${issue.project.name}`,
    issue.cycle && `cycle ${issue.cycle.name ?? issue.cycle.number}`,
    issue.estimate != null && `estimate ${issue.estimate}`,
    issue.labels.nodes.length > 0 && issue.labels.nodes.map((l) => l.name).join(", "),
  ].filter(Boolean);
  const where = `M-l in ${issue.tracker ?? "Linear"}`;
  const list = (title, rows) => (rows.length ? `\n\n## ${title}\n\n${rows.join("\n")}` : "");
  const link = (i) => `**${i.identifier}** ${i.title}${i.state ? ` _(${i.state.name})_` : ""}`;
  const relation = { blocks: "Blocks", duplicate: "Duplicates", related: "Related to", similar: "Similar to" };
  const comments = issue.comments.nodes.slice().sort((a, b) => a.createdAt.localeCompare(b.createdAt));
  const comment = (c, reply) => {
    const head = `${reply ? "↳ " : ""}**${c.user?.name ?? c.botActor?.name ?? "Someone"}** · ${day(c.createdAt)}`;
    const body = diagrams(tidy(c.body), dir, columns, where);
    return reply ? `> ${head}\n>\n${body.split("\n").map((l) => `> ${l}`).join("\n")}` : `${head}\n\n${body}`;
  };
  const threads = comments
    .filter((c) => !c.parent)
    .map((c) => [comment(c, false), ...comments.filter((r) => r.parent?.id === c.id).map((r) => comment(r, true))].join("\n\n"));
  const text = [
    `# ${issue.identifier}: ${issue.title}`,
    `\n\n_${facts.join(" · ")}_`,
    `\n\n_Opened by ${issue.creator?.name ?? "someone"} on ${day(issue.createdAt)}, updated ${day(issue.updatedAt)}${issue.parent ? ` · sub-issue of ${issue.parent.identifier} ${issue.parent.title}` : ""}_`,
    "\n\n---\n\n",
    diagrams(tidy(issue.description), dir, columns, where) || "_No description._",
    list("Sub-issues", issue.children.nodes.map((c) => `- ${link(c)}`)),
    list("Related", issue.relations.nodes.map((r) => `- ${relation[r.type] ?? r.type} ${link(r.relatedIssue)}`)),
    list("Links", issue.attachments.nodes.map((a) => `- [${a.title}](${a.url})`)),
    threads.length ? `\n\n## Comments\n\n${threads.join("\n\n---\n\n")}` : "",
  ].join("");
  fs.writeFileSync(path.join(dir, "pr.md"), linkify(text, dir));
};

// The markers: Private Use characters, which glow lays out as text, one
// column each. OPEN, the link's number in digits of their own, MID, the
// text, CLOSE.
const OPEN = "\u{E000}";
const MID = "\u{E001}";
const CLOSE = "\u{E002}";
const LINK_STYLE = "\x1b[38;5;35;1m";
const plain = (line) => line.replace(/\x1b\][^\x07]*\x07/g, "").replace(/\x1b\[[0-9;]*m/g, "");

// <text> with each Markdown link outside code swapped for its marked text, the
// URLs kept in <dir>/links.json for splice.
const linkify = (text, dir) => {
  const urls = [];
  const out = text
    .split(/(^[ \t]*```[^\n]*\n[\s\S]*?^[ \t]*```[ \t]*$|`[^`\n]+`)/m)
    .map((part, i) =>
      i % 2
        ? part
        : part.replace(/(?<!!)\[([^\]\n]+)\]\(<?([^)\s>]+)>?\)/g, (_, label, url) => {
            urls.push(url);
            const n = [...String(urls.length - 1)].map((d) => String.fromCodePoint(0xe010 + Number(d))).join("");
            return `${OPEN}${n}${MID}${label}${CLOSE}`;
          }),
    )
    .join("");
  fs.writeFileSync(path.join(dir, "links.json"), JSON.stringify(urls));
  return out;
};

// glow's line with each marked text made its link again. A link glow wrapped
// onto the next line is closed at the end of this one and opened again there,
// so the pager never holds one open across lines; and glow's style for each
// word inside it gets the link's colour after it.
const relink = (urls) => {
  let open = "";
  const osc = (url) => `\x1b]8;;${url}\x07`;
  return (line) => {
    let out = open ? osc(open) + LINK_STYLE : "";
    const parts = line.split(/(\u{E000}[\u{E010}-\u{E019}]+\u{E001}|\u{E002}|\x1b\[[0-9;]*m)/u);
    parts.forEach((part, i) => {
      if (i % 2 === 0) out += part;
      else if (part.startsWith(OPEN)) {
        open = urls[Number([...part.slice(1, -1)].map((c) => c.codePointAt(0) - 0xe010).join(""))] ?? "";
        out += osc(open) + LINK_STYLE;
      } else if (part === CLOSE) {
        open = "";
        out += osc("") + "\x1b[22;39m";
      } else out += open ? part + LINK_STYLE : part;
    });
    return open ? out + osc("") : out;
  };
};

const splice = (dir) => {
  const input = fs.readFileSync(0, "utf8");
  const links = fs.existsSync(path.join(dir, "links.json")) ? relink(JSON.parse(fs.readFileSync(path.join(dir, "links.json"), "utf8"))) : (line) => line;
  process.stdout.write(
    input
      .split("\n")
      .map((line) => {
        const n = new RegExp(`${MARK}(\\d+)`).exec(plain(line))?.[1];
        // glow pads every line out to its width with spaces, which less -S
        // marks as cut off once the pane is narrower; they go.
        if (!n) return links(line).replace(/(?:[ \t]|\x1b\[[0-9;]*m)+$/, "\x1b[0m");
        return fs.readFileSync(path.join(dir, `${n}.txt`), "utf8").split("\n").map((l) => `  ${l}`).join("\n");
      })
      .join("\n"),
  );
};

const [mode, ...args] = process.argv.slice(2);
if (mode === "prepare") prepare(args[0], args[1], Number(args[2]) || 80);
else if (mode === "ticket") ticket(args[0], args[1], Number(args[2]) || 80);
else if (mode === "page") webPage(args[0], args[1], Number(args[2]) || 80);
else if (mode === "top") {
  // The first line of the new drawing that starts as <line> did, after a
  // resize, to open it there again; 1 when none does.
  const words = (text) => text.replace(/\s+/g, " ").trim();
  const want = words(args[1] ?? "").slice(0, 40);
  const at = want ? fs.readFileSync(args[0], "utf8").split("\n").findIndex((l) => words(plain(l)).startsWith(want)) : -1;
  process.stdout.write(String(at + 1 || 1));
}
else if (mode === "splice") splice(args[0]);
else {
  console.error("Usage: pr-view.js prepare|ticket <file.json> <dir> <columns> | splice <dir>");
  process.exit(1);
}
