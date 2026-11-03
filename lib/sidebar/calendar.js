// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Mission control's Calendar view: today laid out as Google Calendar's Day
// view lays it out, read straight from your calendar's iCal feed when its
// secret address is in the Keychain (calendar-feed.js), else from the day
// /today last read (its scripts/save.js writes STATE_DIR/calendar.json; the
// Calendar tab, M-p, runs /today every 30 minutes). All-day events across the top, then an hour column, each
// event a colored block from its start to its end with its title and times,
// events at the same time side by side, a red line where it is now, and
// events already over faded. One you haven't said yes to is drawn in outline,
// on the panel's color with a colored edge, as Google Calendar draws it. The
// rows fit the day from 8 AM (or the first event) to 6 PM (or the last) to
// the screen, up to eight to an hour. j and k choose an
// event and Enter joins its call, or opens it in Google Calendar. Drawing
// reads only the file, kept while its modified time is the same.
//
// A day saved before today, or more than STALE_MS ago, is out of date, and
// the view says so rather than passing it off as today's. The daemon then
// opens a Calendar tab in the background (tmux/loop-tab.sh, as M-p's
// command) unless a tab already loops /today, at most once every
// REOPEN_MS, so the view catches up on its own.

const fs = require("node:fs");
const path = require("node:path");
const { STATE_DIR } = require("./config");
const { width, pad, wrap, fg, bg, BOLD, RESET, theme, ago } = require("./text");
const { feed, feedDown } = require("./calendar-feed");
const { CALENDAR } = require("./setup-help");

const FILE = () => path.join(STATE_DIR, "calendar.json");
// Each event's blue, and the deeper one an event takes when it starts just as
// the one above it in its lane ends, so two meetings back to back read as
// two rather than one long block; the same for events already over.
const SOLID = [32, 25];
const PAST = [24, 23];
const CHOSEN = 75;
const NOW = 196;
const STALE = 214;
const STALE_MS = 45 * 60_000;
const REOPEN_MS = 10 * 60_000;

let cache = { mtime: 0, day: undefined };
// The feed's day while it has read today's, else /today's; with the feed
// lost, /today's once the Calendar tab has read today since the feed last did.
const readDay = () => {
  const fromFeed = feed.day?.date === localDate() ? feed.day : undefined;
  if (!fromFeed) return savedDay();
  const saved = feedDown() ? savedDay() : undefined;
  return saved?.date === localDate() && Number(saved.updated ?? 0) > Number(fromFeed.updated ?? 0) ? saved : fromFeed;
};
const savedDay = () => {
  try {
    const mtime = fs.statSync(FILE()).mtimeMs;
    if (mtime !== cache.mtime) cache = { mtime, day: JSON.parse(fs.readFileSync(FILE(), "utf8")) };
  } catch {
    cache = { mtime: 0, day: undefined };
  }
  return cache.day;
};

const localDate = (at = new Date()) => `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
const clock = (at) => new Date(at).toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" }).replace(":00", "");
const range = (start, end) => {
  const [from, to] = [clock(start), clock(end)];
  return from.slice(-2) === to.slice(-2) ? `${from.slice(0, -3)} – ${to}` : `${from} – ${to}`;
};
// Plain text cut to `columns`, an ellipsis where it was cut: text.js's cut
// adds a color reset, which put would draw as cells.
const clip = (text, columns) => {
  let out = "";
  for (const ch of String(text)) {
    if (width(out + ch) > columns - 1 && width(String(text)) > columns) return `${out}…`;
    out += ch;
  }
  return out;
};
const going = (e) => e.you === "going" || e.you === "yours";

// With the feed set up and reading, the daemon keeps it current, so no
// Calendar tab is needed for it. Once it is lost (feedDown), /today keeps the
// day instead, as without a feed: out of date until a Calendar tab has read
// it in the last STALE_MS, which has the daemon open one.
const fresh = (day) => Boolean(day) && day.date === localDate() && Date.now() - Number(day.updated ?? 0) <= STALE_MS;
// With the feed lost, only a saved day the view shows (newer than the
// feed's last) counts as kept.
const calendarStale = (day) => {
  if (!feed.configured) return Boolean(day) && !fresh(day);
  const shown = readDay();
  return feedDown() && !(shown === savedDay() && fresh(shown));
};
// A tab already keeping the calendar: Claude in the Calendar tab, or any tab
// looping /today.
const calendarLooping = (tabs) => tabs.some((t) => (t.name === "Calendar" && t.claude) || /^\/today\b/.test(t.loop?.prompt ?? ""));

// The view, while /today has saved a day: its events in the order the cursor
// walks them, all-day first, then by start. None when it never has.
const calendarView = (tabs = []) => {
  const day = readDay();
  // Nothing to draw yet: there all the same, saying how to set it up, or that
  // the feed is being read, or that the Calendar tab is looking.
  if (!day) {
    const error = feed.configured && !feedDown() ? "Reading your calendar..." : calendarLooping(tabs) ? "Nothing saved yet: the Calendar tab is looking." : feed.configured ? `${feed.error} Opening a Calendar tab to read it with /today.` : CALENDAR;
    return { title: "Calendar", calendar: true, day: undefined, today: true, items: [], loaded: true, builtIn: true, error };
  }
  const today = day.date === localDate();
  const stale = calendarStale(day);
  const looping = calendarLooping(tabs);
  const events = today ? day.events ?? [] : [];
  const items = [...events.filter((e) => e.allDay), ...events.filter((e) => !e.allDay).sort((a, b) => Date.parse(a.start) - Date.parse(b.start))].map((e) => ({
    title: e.title,
    detail: e.allDay ? "all day" : range(e.start, e.end),
    url: e.join || e.link,
    event: e,
  }));
  return { title: "Calendar", calendar: true, day, today, stale, looping, items, loaded: true, builtIn: true };
};

// A row of cells, { ch, fg, bg, bold }, drawn as text: a wide character
// takes its cell and an empty one after it.
const put = (row, x, text, style, limit) => {
  let at = x;
  [...text].forEach((ch) => {
    const w = width(ch);
    if (at + w > limit) return;
    row[at] = { ch, ...style };
    if (w === 2) row[at + 1] = { ch: "", ...style };
    at += w;
  });
};
const draw = (row) => {
  let out = "";
  let last = "";
  row.forEach((cell) => {
    const style = `${cell.bg !== undefined ? bg(cell.bg) : ""}${cell.fg !== undefined ? fg(cell.fg) : ""}${cell.bold ? BOLD : ""}`;
    if (style !== last) {
      out += RESET + style;
      last = style;
    }
    out += cell.ch;
  });
  return `${out}${RESET}`;
};

const calendarLines = (section, cols, body, cursor) => {
  const p = theme();
  const lines = [];
  // No day yet: what the view says about it, whole.
  if (!section.day) {
    const why = feed.configured && feed.error && !feedDown() ? `Couldn't read your calendar: ${feed.error}` : section.error;
    wrap(why ?? "", cols - 4).forEach((line) => lines.push(pad(`  ${fg(STALE)}${line}${RESET}`, cols)));
    return lines;
  }
  const catching = feed.configured && !feedDown() ? feed.error ?? "reading the calendar feed" : section.looping ? "the Calendar tab fetches it every 30 minutes" : "opening a Calendar tab to fetch it";
  if (!section.today) {
    lines.push(pad(`  ${fg(STALE)}No calendar for today yet (last saved ${section.day.date}): ${catching}.${RESET}`, cols));
    return lines;
  }
  const now = Date.now();
  const midnight = new Date(`${section.day.date}T00:00:00`).getTime();
  const minutes = (at) => (Date.parse(at) - midnight) / 60_000;
  const nowMin = (now - midnight) / 60_000;
  const chosen = section.items[cursor]?.event;
  const updated = section.day.updated ? `  updated ${ago(Math.floor(section.day.updated / 1000))} ago` : "";
  const outOfDate = section.stale ? `  ${fg(STALE)}out of date: ${catching}` : feed.configured && feed.error ? `  ${fg(STALE)}${feed.error}` : "";
  const date = new Date(midnight).toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" });
  lines.push(pad(` ${fg(p.bright)}${BOLD}${date}${RESET}${fg(p.faint)}${updated}${outOfDate}${RESET}`, cols));

  const GUTTER = 9;
  const area = cols - GUTTER;
  // All-day events as chips in a row of their own, as Google Calendar
  // shows them above the hours.
  const allDay = section.items.filter((i) => i.event.allDay);
  if (allDay.length) {
    const row = Array.from({ length: cols }, () => ({ ch: " " }));
    put(row, 0, "all day".padStart(GUTTER - 2), { fg: p.dim }, GUTTER);
    let x = GUTTER;
    allDay.forEach((item) => {
      const e = item.event;
      const style = e === chosen ? { bg: CHOSEN, fg: 232, bold: true } : going(e) ? { bg: SOLID[0], fg: 255, bold: true } : { bg: p.panel, fg: SOLID[0] };
      const text = ` ${clip(e.title, Math.max(4, Math.min(30, cols - x - 3)))} `;
      put(row, x, text, style, cols);
      x += width(text) + 1;
    });
    lines.push(draw(row));
  }
  lines.push(" ".repeat(cols));

  const timed = section.items.filter((i) => !i.event.allDay).map((i) => i.event);
  const rows = Math.max(4, body - lines.length);
  const from = Math.floor(Math.min(8 * 60, nowMin, ...timed.map((e) => minutes(e.start))) / 60);
  const to = Math.min(24, Math.ceil(Math.max(18 * 60, nowMin + 1, ...timed.map((e) => minutes(e.end))) / 60));
  // As many rows to an hour as the screen has room for, up to eight, so the
  // day fills it as Google Calendar's does; an hour a row at least.
  const step = 60 / Math.max(1, Math.min(8, Math.floor(rows / (to - from))));
  // A day longer than the screen even by the hour starts an hour before now.
  const first = ((to - from) * 60) / step <= rows ? from : Math.max(from, Math.min(Math.floor(nowMin / 60) - 1, to - rows));
  const count = Math.min(rows, Math.ceil(((to - first) * 60) / step));
  const slot = (m) => (m - first * 60) / step;

  // Events that overlap share the width, each in the first lane free, as
  // Google Calendar places them.
  const placed = timed
    .map((e) => {
      const top = Math.max(0, Math.round(slot(minutes(e.start))));
      return { e, top, bottom: Math.max(top + 1, Math.round(slot(minutes(e.end)))) };
    })
    .filter((b) => b.bottom > 0 && b.top < count)
    .sort((a, b) => a.top - b.top || b.bottom - a.bottom);
  const clusters = [];
  placed.forEach((b) => {
    const last = clusters.at(-1);
    if (last && b.top < last.bottom) {
      last.blocks.push(b);
      last.bottom = Math.max(last.bottom, b.bottom);
    } else clusters.push({ blocks: [b], bottom: b.bottom });
  });
  clusters.forEach(({ blocks }) => {
    const lanes = [];
    blocks.forEach((b) => {
      b.lane = lanes.findIndex((end) => end <= b.top);
      if (b.lane === -1) b.lane = lanes.length;
      lanes[b.lane] = b.bottom;
    });
    blocks.forEach((b) => {
      b.width = Math.floor(area / lanes.length);
      b.x = GUTTER + b.lane * b.width;
    });
  });
  // An event that starts on the row the one above it in the same place ends
  // takes the other shade of that one.
  placed.forEach((b) => {
    const above = placed.find((a) => a !== b && a.x === b.x && a.width === b.width && a.bottom === b.top);
    b.shade = above ? 1 - (above.shade ?? 0) : 0;
  });

  const nowRow = Math.floor(slot(nowMin));
  for (let r = 0; r < count; r++) {
    const at = first * 60 + r * step;
    const hour = at % 60 === 0;
    const row = Array.from({ length: cols }, (_, x) => ({ ch: x >= GUTTER && hour ? "┈" : " ", fg: p.faint }));
    if (hour) put(row, 0, clock(midnight + at * 60_000).padStart(GUTTER - 2), { fg: p.dim }, GUTTER);
    if (r === nowRow) {
      put(row, GUTTER - 1, "●", { fg: NOW }, GUTTER);
      for (let x = GUTTER; x < cols; x++) row[x] = { ch: "─", fg: NOW };
    }
    placed
      .filter((b) => b.top <= r && r < Math.min(b.bottom, count))
      .forEach((b) => {
        const e = b.e;
        const over = Date.parse(e.end) <= now;
        const style = e === chosen ? { bg: CHOSEN, fg: 232 } : going(e) ? { bg: (over ? PAST : SOLID)[b.shade], fg: over ? 250 : 255 } : { bg: p.panel, fg: over ? p.dim : SOLID[b.shade] };
        const w = b.width - 1;
        for (let x = b.x; x < b.x + w; x++) row[x] = { ch: " ", ...style };
        // An outlined event's colored edge.
        if (!going(e) && e !== chosen) row[b.x] = { ch: "▎", ...style, fg: SOLID[b.shade] };
        const line = r - b.top;
        const single = Math.min(b.bottom, count) - b.top === 1;
        const text = single ? `${e.title} · ${range(e.start, e.end)}` : line === 0 ? e.title : line === 1 ? range(e.start, e.end) : "";
        if (text) put(row, b.x + 1, clip(text, w - 2), { ...style, bold: line === 0 }, b.x + w);
      });
    lines.push(draw(row));
  }
  return lines;
};

module.exports = { calendarView, calendarLines, readDay, calendarStale, calendarLooping, REOPEN_MS, range };
