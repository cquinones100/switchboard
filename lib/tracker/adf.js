// Jira's rich text, the Atlassian Document Format (a tree of JSON nodes), as
// the Markdown the ticket view and image draw. Every node Jira's editor makes
// has a rule here; one it doesn't know is read for its text, so nothing is
// lost, only plainer. Attachments have no address Markdown could show, so
// they read as their name.
//
// https://developer.atlassian.com/cloud/jira/platform/apis/document/structure/

const MARKS = {
  strong: (t) => `**${t}**`,
  em: (t) => `_${t}_`,
  strike: (t) => `~~${t}~~`,
  code: (t) => `\`${t}\``,
};

const text = (node) => {
  const marks = node.marks ?? [];
  // Code first, since nothing inside backticks is read as Markdown.
  const ordered = [...marks].sort((a, b) => (b.type === "code") - (a.type === "code"));
  let out = node.text ?? "";
  // Spaces stay outside the marks, where Markdown can read them.
  const [, lead, core, trail] = /^(\s*)([\s\S]*?)(\s*)$/.exec(out);
  if (!core) return out;
  out = core;
  ordered.forEach((mark) => {
    if (MARKS[mark.type]) out = MARKS[mark.type](out);
    else if (mark.type === "link" && mark.attrs?.href) out = `[${out}](${mark.attrs.href})`;
  });
  return lead + out + trail;
};

const inline = (nodes = []) => nodes.map(inlineNode).join("");

const inlineNode = (node) => {
  switch (node.type) {
    case "text":
      return text(node);
    case "hardBreak":
      return "  \n";
    case "mention":
      return node.attrs?.text ? (node.attrs.text.startsWith("@") ? node.attrs.text : `@${node.attrs.text}`) : "@someone";
    case "emoji":
      return node.attrs?.text ?? node.attrs?.shortName ?? "";
    case "inlineCard":
      return node.attrs?.url ? `<${node.attrs.url}>` : "";
    case "status":
      return node.attrs?.text ? `**[${node.attrs.text}]**` : "";
    case "date":
      return node.attrs?.timestamp ? new Date(Number(node.attrs.timestamp)).toISOString().slice(0, 10) : "";
    case "placeholder":
      return "";
    default:
      return node.content ? inline(node.content) : (node.text ?? "");
  }
};

const indent = (block, prefix) =>
  block
    .split("\n")
    .map((line, i) => (i === 0 ? line : line ? " ".repeat(prefix.length) + line : line))
    .join("\n");

// A list item's paragraphs, with a list inside it kept tight against the
// line it belongs under.
const LISTS = new Set(["bulletList", "orderedList", "taskList"]);
const itemBody = (nodes = []) =>
  nodes
    .map((node) => ({ node, text: block(node) }))
    .filter(({ text }) => text !== "")
    .map(({ node, text }, i) => (i === 0 ? text : `${LISTS.has(node.type) ? "\n" : "\n\n"}${text}`))
    .join("");

const list = (node, marker) =>
  (node.content ?? [])
    .map((item, i) => {
      const prefix = marker === "1." ? `${(node.attrs?.order ?? 1) + i}. ` : marker;
      return prefix + indent(itemBody(item.content).trim(), prefix);
    })
    .join("\n");

const tasks = (node) =>
  (node.content ?? [])
    .map((item) =>
      item.type === "taskList" ? indent(`  ${tasks(item)}`, "  ") : `- [${item.attrs?.state === "DONE" ? "x" : " "}] ${inline(item.content)}`,
    )
    .join("\n");

const cell = (node) => blocks(node.content).trim().replace(/\n+/g, " ").replace(/\|/g, "\\|");

const table = (node) => {
  const rows = (node.content ?? []).map((row) => (row.content ?? []).map(cell));
  if (!rows.length) return "";
  const width = Math.max(...rows.map((r) => r.length));
  const pad = (r) => [...r, ...Array(width - r.length).fill("")];
  const [head, ...body] = rows.map(pad);
  return [`| ${head.join(" | ")} |`, `|${" --- |".repeat(width)}`, ...body.map((r) => `| ${r.join(" | ")} |`)].join("\n");
};

const quote = (block) =>
  block
    .split("\n")
    .map((l) => (l ? `> ${l}` : ">"))
    .join("\n");

const block = (node) => {
  switch (node.type) {
    case "paragraph":
      return inline(node.content);
    case "heading":
      return `${"#".repeat(Math.min(Math.max(node.attrs?.level ?? 1, 1), 6))} ${inline(node.content)}`;
    case "bulletList":
      return list(node, "- ");
    case "orderedList":
      return list(node, "1.");
    case "taskList":
      return tasks(node);
    case "decisionList":
      return (node.content ?? []).map((item) => `- ✓ ${inline(item.content)}`).join("\n");
    case "codeBlock":
      return `\`\`\`${node.attrs?.language ?? ""}\n${(node.content ?? []).map((t) => t.text ?? "").join("")}\n\`\`\``;
    case "blockquote":
      return quote(blocks(node.content));
    case "panel":
      return quote(`**${(node.attrs?.panelType ?? "note").replace(/^./, (c) => c.toUpperCase())}:** ${blocks(node.content).trim()}`);
    case "expand":
    case "nestedExpand":
      return [node.attrs?.title ? `**${node.attrs.title}**` : "", blocks(node.content)].filter(Boolean).join("\n\n");
    case "rule":
      return "---";
    case "table":
      return table(node);
    case "blockCard":
    case "embedCard":
      return node.attrs?.url ? `<${node.attrs.url}>` : "";
    case "mediaSingle":
    case "mediaGroup":
      return (node.content ?? [])
        .map((m) => `_[attachment${m.attrs?.alt ? `: ${m.attrs.alt}` : ""}]_`)
        .join(" ");
    case "media":
      return `_[attachment${node.attrs?.alt ? `: ${node.attrs.alt}` : ""}]_`;
    case "layoutSection":
    case "layoutColumn":
    case "bodiedExtension":
      return blocks(node.content);
    case "extension":
    case "inlineExtension":
      return "";
    default:
      return node.content ? blocks(node.content) : inlineNode(node);
  }
};

const blocks = (nodes = []) =>
  nodes
    .map(block)
    .filter((b) => b !== "")
    .join("\n\n");

// Markdown from a document, or from a string (Jira's older API, and a few
// fields, still give plain text).
const markdownOf = (doc) => {
  if (doc == null) return "";
  if (typeof doc === "string") return doc;
  return blocks(doc.content).trim();
};

module.exports = { markdownOf };
