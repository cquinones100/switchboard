// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Frames for the sidebar and mission control: plain functions from what is
// known to the lines to draw, so the tests can read them.

const os = require("node:os");
const { grouped, staged, monitoringOnly, testingDeploy } = require("./model");
const { calendarLines } = require("./calendar");
const { lastTurn, transcriptFor } = require("./claude");
const { width, cut, pad, wrap, layout, plain, fg, bg, BOLD, RESET, STATES, SPINNER, theme, ago } = require("./text");

const home = os.homedir();
// The most lines a tab's name wraps to in the sidebar.
const NAME_LINES = 3;
const tilde = (dir) => (dir && dir.startsWith(home) ? `~${dir.slice(home.length)}` : dir ?? "");

// A PR's checks, review and size in a few marks: "✓12 ✗1 ⏳3", "approved",
// "+340 −80". Failing checks, and changes asked for, first.
const REVIEW_WORDS = { APPROVED: "approved", CHANGES_REQUESTED: "changes requested", REVIEW_REQUIRED: "review required" };
const prFacts = (tab) => {
  const c = tab.checks;
  const checks = c && [c.pass && `✓${c.pass}`, c.fail && `✗${c.fail}`, c.pending && `⏳${c.pending}`].filter(Boolean).join(" ");
  return [checks, REVIEW_WORDS[tab.review], tab.size && `+${tab.size.additions} −${tab.size.deletions}`].filter(Boolean);
};

// The state a tab's badge shows: pinned, whatever else it is, since you
// pinned it yourself and its words still say what Claude is doing (stateWords);
// else Claude's, or, for an idle tab, its PR's place in the merge queue. A
// merged PR's tab shows merged whatever Claude says, unless it waits on you,
// as the Merged group it is listed under does: its work is over, and a tab can
// be left marked working by a hook event that came after its turn ended.
const badgeState = (tab) =>
  tab.pinned ? "pinned"
  : testingDeploy(tab) && tab.state !== "waiting" && tab.state !== "done" ? "deploy"
  : tab.merge === "merged" && tab.state !== "waiting" && tab.state !== "done" ? "merged"
  : tab.state === "idle" && tab.merge ? tab.merge
  : monitoringOnly(tab) ? "monitor"
  : tab.state === "background" && tab.loop ? tab.loop.kind
  : (tab.state === "working" || tab.state === "background") && tab.loopSkill ? tab.loopSkill
  : tab.state;

// The state mark: a colored pill, the spinner turning while Claude works.
const badge = (state, frame) => {
  const s = STATES[state];
  if (!s.color) return `${fg(theme().faint)} ${s.mark} ${RESET}`;
  const mark = ["working", "background", "review", "ci", "queue", "deploy"].includes(state) ? SPINNER[frame % SPINNER.length] : s.mark;
  // A two-column mark (the pin) takes the space after it, so every pill is
  // three columns and the text after it lines up.
  return `${bg(s.color)}${fg(s.ink)}${BOLD} ${mark}${width(mark) > 1 ? "" : " "}${RESET}`;
};

// What a tab is doing, in words: "waiting on you 4m", "finished 2m ago", and
// where its PR is: "in merge queue at 3", "merged".
const mergeWords = (tab) => (tab.merge === "queued" ? `in merge queue${tab.position ? ` at ${tab.position}` : ""}` : tab.merge === "merged" ? "merged" : "");
const stateWords = (tab) => {
  const words = stateOnly(tab);
  return tab.pinned ? ["pinned", words].filter(Boolean).join(" · ") : words;
};
const stateOnly = (tab) => {
  // The Notes tab while Notes is beside another: where it went.
  if (tab.notesAway) return tab.notesOwner ? `Notes is beside tab ${tab.notesOwner.index} · ${tab.notesOwner.name}` : "Notes isn't open";
  const since = ago(tab.since);
  const merge = mergeWords(tab);
  const and = (words) => [words, merge].filter(Boolean).join(" · ");
  if (tab.state === "waiting") return and([`waiting on you${since ? ` ${since}` : ""}`, tab.wait].filter(Boolean).join(" · "));
  if (tab.state === "done") return and(`finished${since ? ` ${since} ago` : ""}`);
  if (tab.state === "working") return and([`working${since ? ` ${since}` : ""}`, tab.loopSkill && STATES[tab.loopSkill]?.words, tab.activity].filter(Boolean).join(" · "));
  if (tab.state === "background" && tab.loop?.kind === "scheduled") {
    const left = tab.loop.at - Date.now();
    // When first: a long prompt is cut at the line's end.
    return and([left > 60_000 ? `next in ${ago(Math.floor((Date.now() - left) / 1000))}` : "next tick due", tab.loop.prompt || "loop"].join(" · "));
  }
  if (tab.state === "background" && tab.loop) return and(tab.loop.kind === "queue" ? "queue-and-clean · watching" : tab.loop.kind === "deploy" ? "testing deploy" : `${STATES[tab.loop.kind].words} · round ${tab.loop.round}`);
  if (monitoringOnly(tab)) return and(["monitoring", ...tab.monitors].join(" · "));
  if (tab.state === "background") return and(`working in background · ${tab.background} task${tab.background === 1 ? "" : "s"}`);
  if (merge) return merge;
  // A Claude tab's program goes without saying; any other's is named.
  const command = tab.claude ? "" : tab.command ?? "";
  return [since && `idle for ${since}`, command].filter(Boolean).join(" · ");
};

const counts = (tabs, p) =>
  ["waiting", "done", "working"]
    // Working counts the tabs working in the background, a loop's too.
    .map((state) => [state, tabs.filter((t) => t.state === state || (state === "working" && t.state === "background")).length])
    .filter(([, n]) => n)
    .map(([state, n]) => `${fg(STATES[state].color)}${BOLD}${STATES[state].mark === "⠋" ? "⋯" : STATES[state].mark} ${n}${RESET}`)
    .join(`${fg(p.faint)}  `);

// The sidebar for one window: its session's tabs, most in need of you first,
// then each plugin's short list. `view` is { focused, cursor, frame, here },
// where here is the window the sidebar sits in and cursor the window chosen
// with j/k while it has focus. Returns the lines, and for each the window it
// belongs to, for a mouse click.
const sidebarLines = ({ tabs, frozen, sections, cols, rows, view, notices = [] }) => {
  const p = theme();
  const lines = [];
  const owners = [];
  let cursorAt = 0;
  let cursorEnd = 0;
  const selected = view.focused ? view.cursor : view.here;
  const here = tabs.find((t) => t.id === view.here);
  const pill = here?.color ? `${bg(here.color)}${fg(232)}${BOLD}` : `${bg(p.highlight)}${fg(p.bright)}${BOLD}`;
  lines.push(pad(` ${pill} ${here?.sessionName ?? ""} ${RESET}  ${counts(tabs, p)}`, cols));
  // A / filter being typed, or kept, in the line under it.
  lines.push(view.filter ? pad(` ${fg(220)}/${view.filter.text}${view.filter.typing ? `${fg(p.dim)}▏` : ""}${RESET}`, cols) : " ".repeat(cols));
  grouped(tabs, frozen).forEach((group) => {
    lines.push(pad(` ${fg(p.dim)}${group.title.toUpperCase()}${fg(p.faint)} ${group.tabs.length}${RESET}`, cols));
    group.tabs.forEach((tab, i) => {
      const chosen = tab.id === selected;
      if (chosen) cursorAt = lines.length;
      const fill = chosen ? bg(p.highlight) : "";
      const bar = `${fg(tab.color ?? p.faint)}${chosen ? "▌" : "▏"}${RESET}${fill}`;
      const nameColor = tab.id === view.here ? `${fg(p.bright)}${BOLD}` : fg(p.text);
      // The name wraps under itself, up to NAME_LINES lines.
      const indent = 6 + String(tab.index).length;
      const wrapped = wrap(tab.name, cols - indent);
      const name = wrapped.length > NAME_LINES ? [...wrapped.slice(0, NAME_LINES - 1), cut(wrapped.slice(NAME_LINES - 1).join(" "), cols - indent)] : wrapped;
      const first = `${bar}${badge(badgeState(tab), view.frame)}${fill} ${fg(p.faint)}${tab.index}${RESET}${fill} ${nameColor}${name[0]}${RESET}${fill}`;
      const rest = name.slice(1).map((line) => `${bar}${" ".repeat(indent - 1)}${nameColor}${line}${RESET}${fill}`);
      // What it is doing, packed onto as few lines as fit, breaking only
      // between its parts, so a PR number is never cut.
      // A failing check needs you, so the sidebar says so too.
      // The PR's number, unless the tab's name already says it.
      const pr = tab.pr && !tab.name.includes(`#${tab.pr}`) && `#${tab.pr}`;
      const parts = [...stateWords(tab).split(" · ").filter(Boolean), pr, tab.checks?.fail && `✗${tab.checks.fail} failing`, tab.notesHere && "✎ Notes here"].filter(Boolean);
      const details = parts.reduce((out, part) => {
        if (out.length && width(`${out.at(-1)} · ${part}`) <= cols - 6) out[out.length - 1] += ` · ${part}`;
        else out.push(part);
        return out;
      }, []);
      const stateColor = STATES[badgeState(tab)].color ?? p.dim;
      const below = details.map((detail) => `${bar}     ${fg(tab.state === "idle" ? p.dim : stateColor)}${detail}${RESET}${fill}`);
      [first, ...rest, ...below].forEach((line) => {
        owners[lines.length] = tab.id;
        lines.push(`${fill}${pad(line, cols)}${RESET}`);
      });
      if (chosen) cursorEnd = lines.length - 1;
      // A faint rule between tabs, which also spaces them apart.
      if (i < group.tabs.length - 1) lines.push(pad(` ${fg(p.highlight)}${"─".repeat(cols - 2)}${RESET}`, cols));
    });
    lines.push(" ".repeat(cols));
  });
  sections.filter((s) => s.sidebar).forEach((section) => {
    const attention = section.items.filter((i) => i.state === "attention").length;
    lines.push(pad(` ${fg(p.dim)}${section.title.toUpperCase()}${fg(p.faint)} ${section.items.length}${attention ? `${RESET} ${fg(220)}${BOLD}! ${attention}` : ""}${RESET}`, cols));
    if (section.error) lines.push(pad(`  ${fg(167)}${section.error}${RESET}`, cols));
    section.items.slice(0, section.sidebar).forEach((item) => {
      lines.push(pad(`  ${itemMark(item)} ${fg(p.text)}${item.title}${RESET}`, cols));
    });
    lines.push(" ".repeat(cols));
  });

  // Notifications, such as new Slack messages, pinned to the bottom under a
  // double rule and their own heading, so they read as no tab and never
  // scroll away; a click on one goes where it says (its owner, notice:<what>).
  const noticeLines = [];
  const noticeOwners = [];
  if (notices.length) {
    noticeLines.push(" ".repeat(cols), pad(` ${fg(220)}${"═".repeat(cols - 2)}${RESET}`, cols), pad(` ${fg(220)}${BOLD}NOTIFICATIONS${RESET}`, cols));
    notices.forEach((n) => {
      // The sidebar's cursor on it, as on a tab.
      const fill = view.focused && view.cursor === n.owner ? bg(p.highlight) : "";
      noticeOwners[noticeLines.length] = n.owner;
      noticeLines.push(`${fill}${pad(`${fill}${fg(220)}${fill ? "▌" : " "}${RESET}${fill} ${fg(220)}●${RESET}${fill} ${fg(p.bright)}${BOLD}${n.title}${RESET}${fill}`, cols)}${RESET}`);
      if (n.detail) {
        noticeOwners[noticeLines.length] = n.owner;
        noticeLines.push(`${fill}${pad(`${fill}    ${fg(p.dim)}${n.detail}${RESET}${fill}`, cols)}${RESET}`);
      }
    });
  }
  // The list scrolls to keep the chosen tab in sight above the notifications.
  const footer = [];
  const room = rows - footer.length - noticeLines.length;
  const top = Math.max(0, Math.min(cursorAt, cursorEnd + 1 - room, lines.length - room));
  const shown = lines.slice(top, top + room);
  while (shown.length < room) shown.push(" ".repeat(cols));
  const owned = owners.slice(top, top + room);
  owned.length = room;
  return { lines: [...shown, ...noticeLines, ...footer], owners: [...owned, ...Array.from({ length: noticeLines.length }, (_, i) => noticeOwners[i])] };
};

const ITEM_STATES = { attention: 220, active: 67, idle: undefined };
const itemMark = (item) => {
  const color = ITEM_STATES[item.state];
  return color ? `${fg(color)}${BOLD}●${RESET}` : `${fg(theme().faint)}○${RESET}`;
};

// Mission control lays tabs out in a grid of cards this size, as many across
// as fit.
const gridShape = (cols, rows, config) => {
  const across = Math.max(1, Math.floor(cols / config.cardWidth));
  const cardWidth = Math.floor(cols / across);
  const down = Math.max(1, Math.floor((rows - 2) / config.cardHeight));
  // Cards grow to fill the screen top to bottom.
  return { across, cardWidth, cardHeight: Math.max(6, Math.floor((rows - 2) / down)), down };
};

// Mission control's tabs laid out in stages (see model.js), packed: each row
// of the screen is a shelf of stage boxes side by side, one box per stage. A
// box takes its cards' width and two columns of border, and a shelf is
// `cols` wide. A stage that fits in the room left on the shelf goes there, in
// one row of cards; one that doesn't starts the next shelf, as wide as it
// needs up to the shelf, in as many rows of cards as it takes. A shelf is as
// tall as its tallest box, and the cards of a shorter box stretch to match.
//
// Returns the tabs in order; the shelves, each { rows, blocks: [{ stage,
// cards, across, rows }] } with cards as indices into that order; and the
// rows of cards the keys move through (j/k move a row, keeping the column),
// each { cards }: a shelf's rows, every box taking part in each, a shorter
// box's card spanning the rows its height covers.
const BORDER = 2;
// A card's width in mission control: a row of `across` cards fits even when
// every card is in a box, and border, of its own.
const missionCardWidth = (cols, config) => {
  const { across } = gridShape(cols, 0, config);
  return Math.max(20, Math.floor(cols / across) - BORDER);
};
const missionGrid = (tabs, cols = Infinity, cardWidth = 1) => {
  const order = [];
  const shelves = [];
  const fit = Math.max(1, Math.floor((cols - BORDER) / cardWidth));
  let shelf;
  let used = 0;
  staged(tabs).forEach((stage) => {
    const n = stage.tabs.length;
    const room = shelf ? Math.floor((cols - used - BORDER) / cardWidth) : 0;
    if (!shelf || room < n) {
      shelves.push((shelf = { rows: 0, blocks: [] }));
      used = 0;
    }
    const across = Math.min(n, fit);
    const rows = Math.ceil(n / across);
    const cards = stage.tabs.map((tab) => order.push(tab) - 1);
    shelf.blocks.push({ stage, cards, across, rows });
    shelf.rows = Math.max(shelf.rows, rows);
    used += BORDER + across * cardWidth;
  });
  const rows = shelves.flatMap((s) =>
    Array.from({ length: s.rows }, (_, i) => ({
      cards: s.blocks.flatMap((b) => {
        const row = Math.min(b.rows - 1, Math.floor((i * b.rows) / s.rows));
        return b.cards.slice(row * b.across, row * b.across + b.across);
      }),
    })),
  );
  return { order, shelves, rows };
};

// A link in a card as the few words that name it: a Markdown link as its
// text, a ticket's or PR's address as its ID or number, any other address as
// its host. A card is too narrow for addresses, which only wrap into noise.
const shortUrl = (url) => {
  const ticket = /linear\.app\/[^/]+\/issue\/([A-Za-z][A-Za-z0-9]*-\d+)|\.atlassian\.net\/browse\/([A-Za-z][A-Za-z0-9]*-\d+)/.exec(url)?.slice(1).find(Boolean);
  if (ticket) return ticket.toUpperCase();
  const pr = /github\.com\/[^/]+\/[^/]+\/(?:pull|issues)\/(\d+)/.exec(url)?.[1];
  if (pr) return `#${pr}`;
  return /^https?:\/\/(?:www\.)?([^/?#]+)/.exec(url)?.[1] ?? url;
};
const unlinked = (text) =>
  text
    .replace(/\[([^\]\n]+)\]\(<?[^)\s>]+>?\)/g, "$1")
    .replace(/<(https?:\/\/[^>\s]+)>/g, (_, url) => shortUrl(url))
    .replace(/https?:\/\/[^\s)>\]]*[^\s)>\].,;:!?'"]/g, (url) => shortUrl(url));

// What a card shows under its border, with a reply of at most `room` lines
// (the end of a longer one), or the whole reply when room is Infinity.
const cardBody = (tab, inner, room) => {
  const p = theme();
  const body = [];
  const stateColor = STATES[badgeState(tab)].color ?? p.dim;
  const facts = [stateWords(tab), tab.pr && `#${tab.pr}`, ...prFacts(tab), tab.claude?.name, tab.notesHere && "✎ Notes here"].filter(Boolean).join(" · ");
  body.push(`${fg(stateColor)}${facts}${RESET}`);
  const turn = tab.claude && transcriptFor(tab.claude) && lastTurn(transcriptFor(tab.claude));
  if (turn) {
    wrap(unlinked(plain(turn.prompt)), inner - 2).slice(0, 2).forEach((line, i) => body.push(`${fg(p.dim)}${i ? "  " : "› "}${line}${RESET}`));
    body.push("");
    // A turn stopped on a tool call is asking to run it, under the reply.
    const asking = turn.pending && tab.state === "waiting" ? wrap(`Asking: ${turn.pending}`, inner).slice(0, 2) : [];
    const reply = turn.reply ? unlinked(plain(turn.reply)) : tab.state === "working" ? `(working${turn.tools ? `, ${turn.tools} tool call${turn.tools === 1 ? "" : "s"} so far` : ""})` : "(no reply yet)";
    const left = room - body.length - (asking.length ? asking.length + 1 : 0);
    const lines = layout(reply, inner);
    // The end of a long reply is where its question or conclusion is.
    const kept = lines.length > left ? ["…", ...lines.slice(lines.length - left + 1)] : lines;
    kept.forEach((line) => body.push(`${fg(p.text)}${line}${RESET}`));
    if (asking.length) body.push("", ...asking.map((line) => `${fg(220)}${BOLD}${line}${RESET}`));
  } else {
    body.push(`${fg(p.dim)}${tilde(tab.path)}${RESET}`);
  }
  return body;
};

// How tall a tab's card would need to be to show all of it.
const cardNeeds = (tab, w) => cardBody(tab, w - 4, Infinity).length + 2;

// A tab's card: its mark and name in the border, what it is doing, and the
// latest turn: your prompt, then what Claude said last or the tool call it is
// stopped on.
const card = (tab, w, h, chosen, frame) => {
  const p = theme();
  const inner = w - 4;
  const border = chosen ? `${fg(tab.color ?? p.bright)}${BOLD}` : fg(p.faint);
  const title = ` ${badge(badgeState(tab), frame)} ${fg(p.dim)}${tab.index}${RESET} ${chosen ? `${fg(p.bright)}${BOLD}` : fg(p.text)}${cut(tab.name, inner - 9)}${RESET} `;
  const top = `${border}╭─${RESET}${title}${border}${"─".repeat(Math.max(0, w - 4 - width(title)))}─╮${RESET}`;
  const body = cardBody(tab, inner, h - 2);
  const rowsOut = body.slice(0, h - 2).map((line) => `${border}│${RESET} ${pad(line, inner)} ${border}│${RESET}`);
  while (rowsOut.length < h - 2) rowsOut.push(`${border}│${RESET}${" ".repeat(w - 2)}${border}│${RESET}`);
  return [top, ...rowsOut, `${border}╰${"─".repeat(w - 2)}╯${RESET}`];
};

// The PR board (prboard.js): a column for each place a PR can stand, side by
// side, each PR two lines, its title and where it is. The column with the
// chosen PR scrolls to keep it in view.
const boardLines = (section, cols, body, cursor) => {
  const p = theme();
  const width_ = Math.floor(cols / section.columns.length);
  let at = 0;
  const columns = section.columns.map((column) => {
    const color = column.title === "Merged" ? STATES.merged.color : column.title === "In merge queue" ? STATES.queued.color : column.title === "Approved" ? STATES.done.color : column.title === "Draft" ? p.dim : STATES.working.color;
    const out = [pad(` ${fg(color)}${BOLD}${column.title.toUpperCase()} ${column.items.length}${RESET}`, width_), pad(` ${fg(color)}${"─".repeat(width_ - 2)}${RESET}`, width_)];
    const cards = column.items.map((item) => {
      const chosen = at++ === cursor;
      const fill = chosen ? bg(p.highlight) : "";
      return [
        `${fill}${pad(` ${itemMark(item)}${fill} ${chosen ? `${fg(p.bright)}${BOLD}` : fg(p.text)}${cut(item.title, width_ - 5)}${RESET}${fill}`, width_ - 1)}${RESET} `,
        `${fill}${pad(`   ${fg(p.dim)}${cut(item.detail ?? "", width_ - 5)}${RESET}${fill}`, width_ - 1)}${RESET} `,
        chosen,
      ];
    });
    // Scrolled to keep the chosen PR in view.
    const room = Math.floor((body - 2) / 2);
    const chosenAt = cards.findIndex((c) => c[2]);
    const start = chosenAt >= room ? chosenAt - room + 1 : 0;
    cards.slice(start, start + room).forEach(([a, b]) => out.push(a, b));
    if (!column.items.length) out.push(pad(`   ${fg(p.faint)}none${RESET}`, width_));
    return out;
  });
  return Array.from({ length: body }, (_, i) => pad(columns.map((c) => c[i] ?? " ".repeat(width_)).join(""), cols));
};

// Mission control: a bar naming the views (the tabs, then each plugin), the
// chosen view, and the keys. `view` is { view, cursor, frame }.
// Returns the lines, with `hits` on them: where a click lands, as
// { y, x0, x1, view } for a view's name in the bar or { y, x0, x1, index } for
// a card or a list's row, 1-based screen cells, for mission control's mouse.
const missionLines = ({ tabs, sections, cols, rows, view, config }) => {
  const p = theme();
  const hits = [];
  const views = [{ title: "Tabs", count: tabs.length }, ...sections.map((s) => ({ title: s.title, count: s.items.length }))];
  // The bar's names, after " MISSION CONTROL  ", each " title count " and a space.
  views.reduce((x, v, i) => {
    const w = width(` ${v.title} ${v.count} `);
    hits.push({ y: 1, x0: x, x1: x + w - 1, view: i });
    return x + w + 1;
  }, 1 + width(" MISSION CONTROL  "));
  // While v is typing a view's name, the view it names is lit in yellow.
  const bar = views
    .map((v, i) =>
      view.jump?.match === i ? `${bg(220)}${fg(232)}${BOLD} ${v.title} ${v.count} ${RESET}`
      : i === view.view ? `${bg(p.highlight)}${fg(p.bright)}${BOLD} ${v.title} ${v.count} ${RESET}`
      : `${fg(p.dim)} ${v.title} ${v.count} ${RESET}`)
    .join(" ");
  // A filter being typed, or kept, is said in the bar, as is a view's name.
  const filter = view.jump ? `  ${fg(220)}view: ${view.jump.text}${fg(p.dim)}▏${RESET}`
    : view.filter ? `  ${fg(220)}/${view.filter.text}${view.filter.typing ? `${fg(p.dim)}▏` : ""}${RESET}` : "";
  const lines = [pad(` ${fg(81)}${BOLD}MISSION CONTROL${RESET}  ${bar}  ${counts(tabs, p)}${filter}`, cols)];
  const body = rows - 2;
  if (view.view === 0) {
    // Each shelf: every stage boxed in its color, titled on its top edge, its
    // cards in rows; a shelf as tall as its tallest box. Cards grow taller to
    // fill the screen when the shelves leave room, and the screen scrolls a
    // shelf at a time to keep the chosen card in view.
    const cardWidth = missionCardWidth(cols, config);
    const { order, shelves } = missionGrid(tabs, cols, cardWidth);
    const cardRows = shelves.reduce((sum, s) => sum + s.rows, 0);
    // Cards are as tall as their shelf's fullest card needs, at least six
    // lines and at most what would fill the screen, so a shelf of quiet tabs
    // stays short rather than stretching empty cards down the screen.
    const most = Math.max(6, config.cardHeight, Math.min(30, Math.floor((body - BORDER * shelves.length) / Math.max(1, cardRows))));
    const shelfHeight = (shelf) => Math.min(most, Math.max(6, ...shelf.blocks.flatMap((b) => b.cards.map((i) => cardNeeds(order[i], cardWidth)))));
    const drawn = [];
    // Each card's cells in `drawn`'s lines, turned into screen cells below.
    const cardAt = [];
    let cursorTop = 0;
    let cursorBottom = 0;
    shelves.forEach((shelf) => {
      const top = drawn.length;
      const height = shelf.rows * shelfHeight(shelf);
      let boxX = 1;
      const boxes = shelf.blocks.map((block) => {
        const x = boxX;
        boxX += block.across * cardWidth + 2;
        let rowTop = top + 1;
        Array.from({ length: block.rows }, (_, r) => {
          const h = r < block.rows - 1 ? Math.floor(height / block.rows) : height - Math.floor(height / block.rows) * (block.rows - 1);
          block.cards.slice(r * block.across, r * block.across + block.across).forEach((i, c) => cardAt.push({ top: rowTop, bottom: rowTop + h - 1, x0: x + 1 + c * cardWidth, x1: x + (c + 1) * cardWidth, index: i }));
          rowTop += h;
        });
        const color = fg(STATES[block.stage.color]?.color ?? p.dim);
        const inner = block.across * cardWidth;
        const title = ` ${block.stage.title.toUpperCase()} ${block.stage.tabs.length} `;
        const shownTitle = cut(title, Math.max(0, inner - 2));
        // Its rows of cards share the shelf's height, the last taking what is left.
        const body = Array.from({ length: block.rows }, (_, r) => {
          const h = r < block.rows - 1 ? Math.floor(height / block.rows) : height - Math.floor(height / block.rows) * (block.rows - 1);
          const cards = block.cards.slice(r * block.across, r * block.across + block.across).map((i) => card(order[i], cardWidth, h, i === view.cursor, view.frame));
          return Array.from({ length: h }, (_, line) => pad(cards.map((x) => x[line]).join(""), inner));
        }).flat();
        return [
          `${color}╭─${BOLD}${shownTitle}${RESET}${color}${"─".repeat(Math.max(0, inner - 1 - width(shownTitle)))}╮${RESET}`,
          ...body.map((line) => `${color}│${RESET}${line}${color}│${RESET}`),
          `${color}╰${"─".repeat(inner)}╯${RESET}`,
        ];
      });
      for (let line = 0; line < height + BORDER; line++) drawn.push(pad(boxes.map((box) => box[line]).join(""), cols));
      if (shelf.blocks.some((b) => b.cards.includes(view.cursor))) {
        cursorTop = top;
        cursorBottom = drawn.length;
      }
    });
    const start = Math.max(0, Math.min(cursorTop, cursorBottom - body));
    lines.push(...drawn.slice(start, start + body));
    // drawn's line n is the screen's row n - start + 2, under the bar.
    cardAt.forEach((c) => {
      for (let line = Math.max(c.top, start); line <= Math.min(c.bottom, start + body - 1); line++) hits.push({ y: line - start + 2, x0: c.x0, x1: c.x1, index: c.index });
    });
    if (!order.length) lines.push(pad(`  ${fg(p.dim)}No tabs.${RESET}`, cols));
  } else if (sections[view.view - 1].calendar) {
    lines.push(...calendarLines(sections[view.view - 1], cols, body - lines.length + 1, view.cursor));
  } else if (sections[view.view - 1].board) {
    const section = sections[view.view - 1];
    // An error, or how to set the view up, whole.
    if (section.error) wrap(section.error, cols - 4).forEach((line) => lines.push(pad(`  ${fg(167)}${line}${RESET}`, cols)));
    if (!section.loaded) lines.push(pad(`  ${fg(p.dim)}Loading…${RESET}`, cols));
    else lines.push(...boardLines(section, cols, body - lines.length + 1, view.cursor));
  } else {
    const section = sections[view.view - 1];
    // An error, or how to set the view up, whole.
    if (section.error) wrap(section.error, cols - 4).forEach((line) => lines.push(pad(`  ${fg(167)}${line}${RESET}`, cols)));
    if (!section.loaded) lines.push(pad(`  ${fg(p.dim)}Loading…${RESET}`, cols));
    // A view may say what its emptiness means (`empty`), as the Slack view
    // says when it last looked.
    else if (!section.items.length && !section.error) lines.push(pad(`  ${fg(p.dim)}${section.empty ?? "Nothing here."}${RESET}`, cols));
    const first = Math.max(0, view.cursor - body + 2);
    const shown = section.items.slice(first, first + body - 1);
    // Rows with cells (the Work map) line up in columns, each as wide as its
    // widest cell on the screen, up to a cap; the title takes what is left.
    const caps = [12, 0, 7, 28, 14];
    const widths = caps.map((cap, c) => Math.min(cap || Infinity, Math.max(0, ...shown.filter((item) => item.cells).map((item) => width(item.cells[c])))));
    widths[1] = Math.max(10, Math.min(widths[1], cols - 12 - widths.reduce((sum, w, c) => sum + (c === 1 ? 0 : w + 2), 0)));
    // Settings line up as name, value, then what the option does.
    const labelWidth = Math.min(32, Math.max(0, ...shown.filter((item) => item.setting).map((item) => width(item.title))));
    shown.forEach((item, i) => {
      const chosen = first + i === view.cursor;
      const fill = chosen ? bg(p.highlight) : "";
      const when = item.time ? ago(Math.floor(item.time / 1000)) : "";
      const name = chosen ? `${fg(p.bright)}${BOLD}` : fg(p.text);
      hits.push({ y: lines.length + 1, x0: 1, x1: cols, index: first + i });
      if (item.group) {
        lines.push(`${fill}${pad(` ${fg(p.dim)}${BOLD}${item.title.toUpperCase()}${RESET}${fill} ${fg(p.faint)}${item.count}`, cols)}${RESET}`);
        return;
      }
      if (item.setting) {
        lines.push(`${fill}${pad(` ${itemMark(item)}${fill} ${pad(`${name}${item.title}${RESET}${fill}`, labelWidth)}  ${pad(`${item.state === "active" ? fg(p.bright) : fg(p.dim)}${item.value}${RESET}${fill}`, 13)} ${fg(p.dim)}${item.detail}`, cols)}${RESET}`);
        return;
      }
      if (item.cells) {
        const cells = item.cells.map((cell, c) => pad(`${c === 1 ? name : c === 0 ? fg(p.text) : fg(p.dim)}${cell}${RESET}${fill}`, widths[c])).join("  ");
        lines.push(`${fill}${pad(` ${itemMark(item)}${fill} ${cells}${chosen && item.more ? `  ${fg(p.dim)}${item.more}` : ""}`, cols)}${RESET}`);
        return;
      }
      const left = `${fill} ${itemMark(item)}${fill} ${name}${item.title}${RESET}${fill}  ${fg(p.dim)}${item.detail ?? ""}`;
      lines.push(`${fill}${pad(left, cols - 6)}${fg(p.faint)}${when.padStart(5)} ${RESET}`);
    });
  }
  while (lines.length < rows - 1) lines.push(" ".repeat(cols));
  const listed = sections[view.view - 1];
  const hints = view.jump ? "type a view's name  ↵ go there  esc cancel" : view.view === 0 ? "hjkl move  ↵ actions (↵↵ go to tab)  / filter  tab next view  ? keys  esc close" : listed?.settings ? "jk move  ↵ change  / search  tab next view  ? keys  esc close" : listed?.calendar ? "jk event  hl beside  ↵ actions  tab next view  ? keys  esc close" : listed?.slack ? "jk move  ↵ actions  tab next view  ? keys  esc close" : listed?.title === "Closed" ? "jk move  ↵ actions  tab next view  ? keys  esc close" : listed?.board ? "hl column  jk move  ↵ what to do with it  tab next view  ? keys  esc close" : listed?.items.some((i) => i.group) ? "jk move  ↵ actions, or show merged  tab next view  ? keys  esc close" : "jk move  ↵ actions  r refresh  tab next view  ? keys  esc close";
  const out = [...lines.slice(0, rows - 1), pad(view.note ? ` ${fg(p.bright)}${view.note}${RESET}` : ` ${fg(p.dim)}${hints}${RESET}`, cols)];
  out.hits = hits.filter((h) => h.y < rows);
  return out;
};

// Answering a tab from mission control (see answer.js), over the whole
// frame: its question and options, or, with no prompt, a message to type.
// `answering` is { tab, found, text, typing, note }.
// A Slack message as Slack's app draws it, as near as a terminal can, in
// lines no wider than `cols`: *bold*, _italic_, ~strike~ and `code` styled,
// links and mentions as their text, bullets and numbered items hanging their
// wrapped lines under their text, > quotes behind a bar, and ``` blocks dim
// and kept as they are. Each line ends with the style reset, so the caller can
// put it anywhere.
const slackLines = (text, cols, base = "") => {
  const p = theme();
  const unescape = (s) => s.replace(/&lt;/g, "<").replace(/&gt;/g, ">").replace(/&amp;/g, "&");
  // Words carry their style, so wrapping keeps it.
  const words = (s) => {
    const out = [];
    const style = { b: false, i: false, s: false, c: false };
    const on = () => `${base}${style.b ? BOLD : ""}${style.i ? "\x1b[3m" : ""}${style.s ? "\x1b[9m" : ""}${style.c ? fg(p.accent ?? 180) : ""}`;
    let word = "";
    let styled = "";
    const flush = () => {
      if (word) out.push({ text: word, styled });
      word = "";
      styled = "";
    };
    const plain = unescape(s.replace(/<@[A-Z0-9]+\|([^>]+)>/g, "@$1").replace(/<#[A-Z0-9]+\|([^>]+)>/g, "#$1").replace(/<(https?:[^|>]+)\|([^>]+)>/g, "$2").replace(/<(https?:[^>]+)>/g, "$1"));
    [...plain].forEach((ch, i, all) => {
      const before = all[i - 1] ?? " ";
      const after = all[i + 1] ?? " ";
      const marker = { "*": "b", _: "i", "~": "s", "`": "c" }[ch];
      // A marker opens before a word and closes after one, as Slack's do.
      if (marker && (!style.c || marker === "c")) {
        const opens = !style[marker] && /[\s([{]/.test(before) && !/\s/.test(after);
        const closes = style[marker] && !/\s/.test(before) && /[\s.,;:!?)\]}'"]/.test(after);
        if (opens || closes) {
          style[marker] = opens;
          styled += `${RESET}${on()}`;
          return;
        }
      }
      if (/\s/.test(ch)) {
        flush();
        return;
      }
      if (!word) styled = on();
      word += ch;
      styled += ch;
    });
    flush();
    return out;
  };
  // Lays words out under a first-line lead and a lead for the lines after.
  const lay = (list, lead, hang) => {
    const lines = [];
    let line = "";
    let room = cols - width(lead);
    let used = 0;
    let first = true;
    list.forEach(({ text, styled }) => {
      const w = width(text);
      if (used && used + 1 + w > room) {
        lines.push(`${first ? lead : hang}${line}${RESET}`);
        first = false;
        line = "";
        used = 0;
        room = cols - width(hang);
      }
      line += `${used ? `${RESET}${base} ` : ""}${styled}`;
      used += (used ? 1 : 0) + w;
    });
    lines.push(`${first ? lead : hang}${line}${RESET}`);
    return lines;
  };
  const out = [];
  let fence = false;
  String(text ?? "").split("\n").forEach((raw) => {
    if (/^\s*```/.test(raw)) {
      fence = !fence;
      const rest = raw.replace(/^\s*```/, "").replace(/```\s*$/, "");
      if (rest) out.push(`${fg(p.dim)}  ${cut(unescape(rest), cols - 2)}${RESET}`);
      if (/```\s*$/.test(raw.replace(/^\s*```/, "")) && raw.trim().length > 3) fence = false;
      return;
    }
    if (fence) return out.push(`${fg(p.dim)}  ${cut(unescape(raw), cols - 2)}${RESET}`);
    if (!raw.trim()) return out.push("");
    const bullet = /^(\s*)(?:[•◦▪*-])\s+(.*)$/.exec(raw);
    const number = /^(\s*)(\d+[.)])\s+(.*)$/.exec(raw);
    const quote = /^\s*(?:>|&gt;)\s?(.*)$/.exec(raw);
    if (bullet) {
      const indent = " ".repeat(Math.min(6, bullet[1].length));
      return out.push(...lay(words(bullet[2]), `${indent}${base}• `, `${indent}  `));
    }
    if (number) {
      const indent = " ".repeat(Math.min(6, number[1].length));
      const lead = `${number[2]} `;
      return out.push(...lay(words(number[3]), `${indent}${base}${lead}`, `${indent}${" ".repeat(width(lead))}`));
    }
    if (quote) return out.push(...lay(words(quote[1]), `${fg(p.dim)}▎ ${RESET}`, `${fg(p.dim)}▎ ${RESET}`));
    out.push(...lay(words(raw.trim()), "", ""));
  });
  // No more than one blank line together, none at either end.
  return out.filter((l, i, all) => l || (i > 0 && all[i - 1] && i < all.length - 1));
};

// The sidebar's notifications as a list of their own, in place of its tabs:
// each item (a Slack message, a Linear inbox item, the next meeting) with its
// whole message, wrapped, the chosen one marked; its first line goes back to
// the tabs. `owners` says what each line is: "back", or an item's index.
const noticeListLines = (items, cursor, cols, rows, note) => {
  const p = theme();
  const lines = [pad(` ${fg(p.dim)}‹ tabs  ${fg(220)}${BOLD}NOTIFICATIONS ${items.length}${RESET}`, cols), pad(` ${fg(220)}${"═".repeat(Math.max(0, cols - 2))}${RESET}`, cols)];
  const owners = ["back", "back"];
  let cursorAt = 0;
  let cursorEnd = 0;
  items.forEach((item, i) => {
    const chosen = i === cursor;
    const fill = chosen ? bg(p.highlight) : "";
    if (chosen) cursorAt = lines.length;
    owners[lines.length] = i;
    lines.push(`${fill}${pad(`${fill}${fg(220)}${chosen ? "▌" : " "}${RESET}${fill} ${fg(p.bright)}${BOLD}${cut(item.title, cols - 3)}${RESET}${fill}`, cols)}${RESET}`);
    slackLines(String(item.body ?? "").trim(), cols - 4, `${fill}${fg(p.text)}`).forEach((l) => {
      owners[lines.length] = i;
      lines.push(`${fill}${pad(`${fill}   ${fg(p.text)}${l}${RESET}${fill}`, cols)}${RESET}`);
    });
    if (chosen) cursorEnd = lines.length - 1;
    owners[lines.length] = i;
    lines.push(" ".repeat(cols));
  });
  if (!items.length) lines.push(pad(`   ${fg(p.dim)}Nothing new.${RESET}`, cols));
  // What the last action said, or is doing ("Open it in Slack…"), until the
  // next key; else the keys.
  const footer = pad(note ? ` ${fg(220)}${cut(note, cols - 2)}${RESET}` : ` ${fg(p.dim)}jk move  ↵ actions  esc back${RESET}`, cols);
  // The two heading lines stay; the list scrolls to keep the chosen one in sight.
  const room = rows - 3;
  const body = lines.slice(2);
  const top = Math.max(0, Math.min(cursorAt - 2, cursorEnd - 2 + 1 - room, body.length - room));
  const shown = body.slice(top, top + room);
  while (shown.length < room) shown.push(" ".repeat(cols));
  const shownOwners = owners.slice(2).slice(top, top + room);
  shownOwners.length = room;
  return { lines: [...lines.slice(0, 2), ...shown, footer], owners: [...owners.slice(0, 2), ...shownOwners, undefined] };
};

const answerLines = ({ tab, found, text, typing, note }, cols, rows) => {
  const p = theme();
  const lines = [pad(` ${fg(81)}${BOLD}ANSWER${RESET}  ${fg(p.dim)}${tab.index}${RESET} ${fg(p.bright)}${BOLD}${cut(tab.name, cols - 16)}${RESET}`, cols), " ".repeat(cols)];
  if (found) {
    found.question.split("\n").forEach((line) => wrap(line, cols - 4).forEach((w) => lines.push(pad(`  ${fg(p.bright)}${w}${RESET}`, cols))));
    lines.push(" ".repeat(cols));
    found.options.forEach((o) => {
      const box = o.checked === undefined ? "" : o.checked ? "☑ " : "☐ ";
      lines.push(pad(`  ${fg(220)}${BOLD}${o.n}${RESET}  ${fg(p.text)}${box}${cut(o.label, cols - 10)}${RESET}`, cols));
    });
  } else {
    lines.push(pad(`  ${fg(p.dim)}No question showing: what you type is sent to it as a message.${RESET}`, cols));
  }
  lines.push(" ".repeat(cols));
  if (typing || !found) lines.push(pad(`  ${fg(p.bright)}› ${text}${fg(p.dim)}▏${RESET}`, cols));
  if (note) lines.push(" ".repeat(cols), pad(`  ${fg(p.dim)}${note}${RESET}`, cols));
  while (lines.length < rows - 1) lines.push(" ".repeat(cols));
  const keys = typing || !found ? "type  ↵ send  esc back" : `1-${found.options.length} choose${found.multi ? "  s submit" : ""}${found.free ? "  t type your own" : ""}  esc back`;
  return [...lines.slice(0, rows - 1), pad(` ${fg(p.dim)}${keys}${RESET}`, cols)];
};

// A look at a tab without going there (Space in mission control), over the
// whole frame: its last turn in full (your prompt, all of Claude's reply, and
// the tool call it waits on), or its screen as last captured. `peek` is
// { tab, mode: "turn" | "screen", screen, scroll }; returns the lines and how
// far it can scroll.
const peekBody = ({ tab, mode, screen }, cols) => {
  const p = theme();
  if (mode === "screen") return (screen ?? "").split("\n").map((line) => `${fg(p.text)}${cut(line, cols - 2)}${RESET}`);
  const turn = tab.claude && transcriptFor(tab.claude) && lastTurn(transcriptFor(tab.claude));
  if (!turn) return [`${fg(p.dim)}No Claude turn here; s shows its screen.${RESET}`];
  const body = wrap(plain(turn.prompt), cols - 4).map((line, i) => `${fg(p.dim)}${i ? "  " : "› "}${line}${RESET}`);
  body.push("");
  layout(plain(turn.reply ?? (tab.state === "working" ? "(working)" : "(no reply yet)")), cols - 2).forEach((w) => body.push(`${fg(p.text)}${w}${RESET}`));
  if (turn.pending) body.push("", ...wrap(`Asking: ${turn.pending}`, cols - 2).map((line) => `${fg(220)}${BOLD}${line}${RESET}`));
  return body;
};
const peekLines = (peek, cols, rows) => {
  const p = theme();
  const body = peekBody(peek, cols);
  const room = rows - 3;
  const most = Math.max(0, body.length - room);
  const scroll = Math.max(0, Math.min(peek.scroll ?? most, most));
  const lines = [pad(` ${fg(81)}${BOLD}PEEK${RESET}  ${fg(p.dim)}${peek.tab.index}${RESET} ${fg(p.bright)}${BOLD}${cut(peek.tab.name, cols - 30)}${RESET}  ${fg(p.dim)}${peek.mode === "screen" ? "screen" : "last turn"}${RESET}`, cols), " ".repeat(cols)];
  body.slice(scroll, scroll + room).forEach((line) => lines.push(pad(` ${line}`, cols)));
  while (lines.length < rows - 1) lines.push(" ".repeat(cols));
  lines.push(pad(` ${fg(p.dim)}jk scroll  s ${peek.mode === "screen" ? "last turn" : "screen"}  r capture again  ↵ go to tab  esc back${RESET}`, cols));
  return { lines: lines.slice(0, rows), most };
};

// A line to type (a file's path, a repository's URL): `asking` is { title,
// hint, text }.
const askModal = ({ title, hint, text }, cols, rows) => {
  const p = theme();
  const inner = Math.min(cols - 4, 90);
  const lines = [...wrap(hint, inner - 2).map((w) => [` ${fg(p.dim)}${w}`, ""]), ["", ""], [` ${fg(p.bright)}› ${text}${fg(p.dim)}▏`, ""]];
  return modalBox({ title, lines, keys: "type  ↵ done  esc back", width: inner }, cols, rows);
};

// A question before something that can't be undone, over the whole frame:
// `confirm` is { text, list }.
const confirmLines = ({ text, list = [] }, cols, rows) => {
  const p = theme();
  const lines = [" ".repeat(cols), pad(`  ${fg(220)}${BOLD}${text}${RESET}`, cols), " ".repeat(cols), ...list.map((item) => pad(`    ${fg(p.text)}${cut(item, cols - 6)}${RESET}`, cols))];
  while (lines.length < rows) lines.push(" ".repeat(cols));
  return lines.slice(0, rows);
};

// A box over the middle of the view, which stays in sight around it, as the ?
// keys box does: its title in the top edge, its lines (each a [text, fill]
// pair, fill a background for the whole line or ""), a blank line, then its
// keys. Written after the frame, each line put in place.
const modalBox = ({ title, lines, keys, color = 81, width: wanted = 36 }, cols, rows) => {
  const p = theme();
  const hint = ` ${fg(p.dim)}${keys}`;
  const inner = Math.min(cols - 4, Math.max(wanted, width(title) + 6, ...[...lines.map(([text]) => text), hint].map((line) => width(line) + 2)));
  const shown = lines.slice(0, Math.max(1, rows - 6));
  const top = Math.max(1, Math.floor((rows - shown.length - 4) / 2) + 1);
  const left = Math.max(1, Math.floor((cols - inner - 2) / 2) + 1);
  const edge = fg(color);
  const heading = ` ${cut(title, inner - 4)} `;
  const box = [
    `${edge}╭─${BOLD}${fg(p.bright)}${heading}${RESET}${edge}${"─".repeat(Math.max(0, inner - width(heading) - 1))}╮`,
    ...shown.map(([text, fill]) => `${edge}│${RESET}${fill}${pad(text, inner)}${RESET}${edge}│`),
    `${edge}│${RESET}${" ".repeat(inner)}${edge}│`,
    `${edge}│${RESET}${pad(hint, inner)}${RESET}${edge}│`,
    `${edge}╰${"─".repeat(inner)}╯`,
  ];
  return box.map((line, i) => `\x1b[${top + i};${left}H${line}${RESET}`).join("");
};

// What to do with the item chosen in a view: `choose` is { title, body,
// choices: [{ key, label }], cursor }, body the item's own text (a Slack
// message's), shown above the choices, wrapped. The box grows with what it
// holds, as tall as the pane allows (modalBox's rows - 6): the options come
// first, every one shown, or, when even they don't fit, those around the
// chosen one; the message takes the room left, cut short with … only then.
const chooseModal = ({ title, body, choices, cursor }, cols, rows) => {
  const p = theme();
  const inner = Math.min(cols - 6, 72);
  // A Slack message as Slack draws it (slackLines); other text as it is.
  const said = String(body ?? "").trim() ? slackLines(String(body).trim(), inner - 2, fg(p.text)) : [];
  const room = Math.max(1, rows - 6);
  const fits = Math.min(choices.length, room);
  const first = Math.max(0, Math.min(cursor - Math.floor(fits / 2), choices.length - fits));
  const bodyRoom = room - fits - 1;
  const shown = bodyRoom < 1 ? [] : said.length > bodyRoom ? [...said.slice(0, bodyRoom - 1), "…"] : said;
  const text = shown.length ? [...shown.map((line) => [` ${fg(p.text)}${line}`, ""]), ["", ""]] : [];
  const lines = choices.slice(first, first + fits).map((c, j) => {
    const i = first + j;
    const fill = i === cursor ? bg(p.highlight) : "";
    return [`${fill} ${i === cursor ? `${fg(220)}${BOLD}›${RESET}${fill} ${fg(p.bright)}${BOLD}` : `  ${fg(p.text)}`}${c.label}`, fill];
  });
  return modalBox({ title, lines: [...text, ...lines], keys: "jk move  ↵ choose  esc back", width: text.length ? inner : undefined }, cols, rows);
};

// A question before something that can't be undone: `confirm` is { text,
// list }.
const confirmModal = ({ text, list = [] }, cols, rows) => {
  const p = theme();
  const lines = [[` ${fg(220)}${BOLD}${text}`, ""], ...(list.length ? [["", ""]] : []), ...list.map((item) => [`   ${fg(p.text)}${cut(item, cols - 12)}`, ""])];
  return modalBox({ title: "Are you sure?", lines, keys: "y or ↵ yes  any other key no", color: 220 }, cols, rows);
};

// Answering a tab (a in mission control): the question it shows and its
// options, or a message to send it, typed in the box.
const answerModal = ({ tab, found, text, typing, note }, cols, rows) => {
  const p = theme();
  const inner = Math.min(cols - 4, 90);
  const lines = [];
  if (found) {
    found.question.split("\n").forEach((line) => wrap(line, inner - 2).forEach((w) => lines.push([` ${fg(p.bright)}${w}`, ""])));
    lines.push(["", ""]);
    found.options.forEach((o) => {
      const box = o.checked === undefined ? "" : o.checked ? "☑ " : "☐ ";
      lines.push([` ${fg(220)}${BOLD}${o.n}${RESET}  ${fg(p.text)}${box}${cut(o.label, inner - 8)}`, ""]);
    });
  } else {
    lines.push([` ${fg(p.dim)}No question showing: what you type is sent to it as a message.`, ""]);
  }
  if (typing || !found) lines.push(["", ""], [` ${fg(p.bright)}› ${text}${fg(p.dim)}▏`, ""]);
  if (note) lines.push(["", ""], [` ${fg(p.dim)}${note}`, ""]);
  const keys = typing || !found ? "type  ↵ send  esc back" : `1-${found.options.length} choose${found.multi ? "  s submit" : ""}${found.free ? "  t type your own" : ""}  esc back`;
  return modalBox({ title: `Answer ${tab.index} ${tab.name}`, lines, keys, width: inner }, cols, rows);
};

// The keys each view takes, for its ? screen: [keys, what they do].
const HELP = {
  sidebar: [
    ["j k", "show a tab"],
    ["g G", "first, last"],
    ["↵ l", "go to the tab"],
    ["h esc q ^c", "back to your pane"],
    ["1-9", "go to that tab"],
    ["/", "find a tab by title (↵ go, esc clear)"],
    ["tab", "mission control"],
    ["space", "peek at it"],
    ["a", "answer it"],
    ["d L", "its diff (diffview), lazygit"],
    ["n", "to the notifications, when there are any"],
    ["t", "new tab running Claude there (M-t)"],
    ["M-x C-x", "run that key there"],
    ["?", "these keys"],
  ],
  // Mission control's keys only move about; everything else is in the menu
  // Enter opens on what is chosen.
  mission: [
    ["h j k l", "choose a card or row; in the Calendar, h l move between events side by side"],
    ["g G", "first, last"],
    ["↵", "what can be done with it (on a tab: go there first, so ↵↵ goes)"],
    ["↵ on Merged", "Work map: show or hide the merged worktrees"],
    ["/", "filter the cards (↵ keep, esc clear)"],
    ["1-9", "choose that tab"],
    ["tab ] [", "next, last view"],
    ["v", "go to a view by typing its name (↵ go, esc cancel)"],
    ["r", "refresh a plugin's view"],
    ["esc q", "close"],
    ["M-w", "close, whatever is open over it"],
    ["?", "these keys"],
  ],
};

// A view's keys, over the whole frame, until the next key.
const helpLines = (kind, cols, rows) => {
  const p = theme();
  const keys = HELP[kind] ?? [];
  const keyWidth = Math.max(...keys.map(([k]) => width(k)));
  const lines = [pad(` ${fg(81)}${BOLD}KEYS${RESET}`, cols), " ".repeat(cols)];
  keys.forEach(([k, what]) => lines.push(pad(` ${fg(p.bright)}${BOLD}${k.padEnd(keyWidth)}${RESET}  ${fg(p.text)}${what}${RESET}`, cols)));
  lines.push(" ".repeat(cols), pad(` ${fg(p.dim)}any key to go back${RESET}`, cols));
  while (lines.length < rows) lines.push(" ".repeat(cols));
  return lines.slice(0, rows);
};

// The keys as a box drawn over the view, in its middle: written after the
// frame, each line put in place, so the view stays where it was around it.
const helpModal = (kind, cols, rows) => {
  const p = theme();
  const keys = HELP[kind] ?? [];
  const keyWidth = Math.max(...keys.map(([k]) => width(k)));
  const body = [...keys.map(([k, what]) => ` ${fg(p.bright)}${BOLD}${k.padEnd(keyWidth)}${RESET}  ${fg(p.text)}${what}`), "", ` ${fg(p.dim)}any key to go back`];
  const inner = Math.min(cols - 4, Math.max(28, ...body.map((line) => width(line) + 1)));
  const shown = body.slice(0, Math.max(1, rows - 4));
  const top = Math.max(1, Math.floor((rows - shown.length - 2) / 2) + 1);
  const left = Math.max(1, Math.floor((cols - inner - 2) / 2) + 1);
  const edge = fg(81);
  const title = " Keys ";
  const box = [
    `${edge}╭─${BOLD}${title}${RESET}${edge}${"─".repeat(Math.max(0, inner - title.length - 1))}╮`,
    ...shown.map((line) => `${edge}│${RESET}${pad(line, inner)}${RESET}${edge}│`),
    `${edge}╰${"─".repeat(inner)}╯`,
  ];
  return box.map((line, i) => `\x1b[${top + i};${left}H${line}${RESET}`).join("");
};

// A frame as written to the terminal: cursor and wrapping off, each line put
// in place, so it can be written over the last one as it is.
const frame = (lines, title) =>
  `${title ? `\x1b]2;${title}\x07` : ""}\x1b[?25l\x1b[?7l${lines.map((line, i) => `\x1b[${i + 1};1H${line}${RESET}`).join("")}`;

module.exports = { slackLines, noticeListLines, unlinked, confirmLines, confirmModal, answerModal, askModal, chooseModal, prFacts, HELP, helpLines, helpModal, answerLines, peekLines, missionGrid, missionCardWidth, sidebarLines, missionLines, gridShape, frame, stateWords, tilde };
