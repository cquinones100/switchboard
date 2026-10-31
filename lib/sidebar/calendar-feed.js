// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Today's events read straight from your Google Calendar's secret address in
// iCal format, for the Calendar view (calendar.js), so it needs no Claude
// tab. The address is in the Keychain (service google-calendar-ical), never
// in a file: Google Calendar, Settings, your calendar, Integrate calendar,
// "Secret address in iCal format". Fetched every FEED_POLL_MS in the
// background, and when you go to the view; drawing reads only the last
// answer, in the shape /today saves, so the view takes either. Without the
// address, or while the feed can't be read, the view falls back to what
// /today saved.
//
// ical.js (lib/package.json, pinned) works out the day's occurrences
// of repeating events, moved ones included, in the feed's own time zones.
// Your reply is the PARTSTAT of the attendee who is the calendar's owner
// (the feed's X-WR-CALNAME, its address); an event with no attendees is
// yours. Declined and cancelled events are left out. The call is Google's
// X-GOOGLE-CONFERENCE, else a Zoom, Meet or Teams address in the location or
// description; the event's page is built from its UID as Google builds its
// links, a split series' _R part left out as Google leaves it.

const { execFile } = require("node:child_process");

const FEED_POLL_MS = 5 * 60_000;
const SERVICE = "google-calendar-ical";
const CALL = /https:\/\/[\w.-]*(zoom\.us\/j|meet\.google\.com|teams\.microsoft\.com\/l\/meetup-join)\/[^\s"<>\\]+/;

const feed = { day: undefined, error: undefined, configured: undefined, fetchedAt: 0 };

// The tickets and pull requests an event's description or location names, for
// the next meeting's notification and the Calendar view's menu: Linear, Jira
// and GitHub links, each with where it goes, then ticket IDs written bare (ABC-123),
// which have no link, at most MAX_REFS.
const MAX_REFS = 4;
const refsFrom = (text) => {
  const raw = String(text ?? "");
  // Links are found in the raw text, an HTML link's address being inside its
  // tag; bare IDs in the words alone.
  const plain = raw.replace(/<[^>]+>/g, " ").replace(/https?:\/\/\S+/g, " ");
  const refs = [];
  const add = (ref) => refs.length < MAX_REFS && !refs.some((r) => r.id === ref.id) && refs.push(ref);
  [...raw.matchAll(/https:\/\/(?:linear\.app\/[\w-]+\/issue|[\w-]+\.atlassian\.net\/browse)\/([A-Za-z][A-Za-z0-9]*-\d+)[^\s"<>]*/g)].forEach((m) => add({ id: m[1].toUpperCase(), url: m[0] }));
  [...raw.matchAll(/https:\/\/github\.com\/([\w.-]+)\/([\w.-]+)\/pull\/(\d+)/g)].forEach((m) => add({ id: `${m[2]}#${m[3]}`, url: m[0] }));
  [...plain.matchAll(/(?<![\w/-])([A-Z][A-Z0-9]{1,9}-\d{1,6})(?![\w-])/g)].filter((m) => !/^UTF-|^ISO-|^SHA-/.test(m[1])).forEach((m) => add({ id: m[1] }));
  return refs;
};

const secret = () =>
  new Promise((resolve) => {
    execFile("security", ["find-generic-password", "-s", SERVICE, "-w"], { encoding: "utf8", timeout: 5_000 }, (err, stdout) => resolve(err ? undefined : stdout.trim() || undefined));
  });

const localDate = (at = new Date()) => `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;

// Google's link to an event, or to one occurrence of a repeating one: the
// base64 of its ID (the UID before @google.com, then _ and the occurrence's
// UTC start) and the calendar's address.
// The event's ID in Google's API, which RSVP answers by (respond_to_event).
const idOf = (uid, start, allDay, recurring) => {
  if (!uid?.endsWith("@google.com")) return undefined;
  const iso = start.toISOString().replace(/[-:]/g, "").replace(/\.\d{3}/, "");
  // A series split in two ("this and following") has _R and the split's
  // time after its ID in the feed, which Google's links leave out.
  return `${uid.replace(/@google\.com$/, "").replace(/_R\d{8}(T\d{6}Z?)?$/, "")}${recurring ? `_${allDay ? iso.slice(0, 8) : iso}` : ""}`;
};
const linkOf = (uid, owner, start, allDay, recurring) => {
  if (!uid?.endsWith("@google.com") || !owner) return undefined;
  const id = idOf(uid, start, allDay, recurring);
  return `https://www.google.com/calendar/event?eid=${Buffer.from(`${id} ${owner}`).toString("base64").replace(/=+$/, "")}`;
};

const youOf = (vevent, owner) => {
  const me = vevent.getAllProperties("attendee").find((a) => String(a.getFirstValue()).toLowerCase() === `mailto:${owner}`.toLowerCase());
  if (!me) return "yours";
  return { ACCEPTED: "going", TENTATIVE: "maybe", DECLINED: "declined" }[String(me.getParameter("partstat")).toUpperCase()] ?? "not answered";
};

// Today's events from the feed's text, in /today's shape.
const dayFrom = (text, now = new Date()) => {
  const ICAL = require("ical.js");
  const root = new ICAL.Component(ICAL.parse(text));
  root.getAllSubcomponents("vtimezone").forEach((tz) => ICAL.TimezoneService.register(tz));
  const owner = String(root.getFirstPropertyValue("x-wr-calname") ?? "").includes("@") ? String(root.getFirstPropertyValue("x-wr-calname")) : undefined;
  const dayStart = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const dayEnd = new Date(dayStart.getTime() + 24 * 60 * 60_000);
  const vevents = root.getAllSubcomponents("vevent");
  const masters = new Map();
  vevents.filter((v) => !v.hasProperty("recurrence-id")).forEach((v) => masters.set(v.getFirstPropertyValue("uid"), new ICAL.Event(v)));
  // A moved or changed occurrence: its master's, or one of its own when the
  // feed has no master for it.
  vevents.filter((v) => v.hasProperty("recurrence-id")).forEach((v) => {
    const master = masters.get(v.getFirstPropertyValue("uid"));
    if (master) master.relateException(v);
    else masters.set(`${v.getFirstPropertyValue("uid")}#${v.getFirstPropertyValue("recurrence-id")}`, new ICAL.Event(v));
  });
  const events = [];
  // `original` is when a moved occurrence was meant to start, which Google
  // still knows it by.
  const add = (event, start, end, recurring, original = start) => {
    const vevent = event.component;
    if (String(vevent.getFirstPropertyValue("status") ?? "").toUpperCase() === "CANCELLED") return;
    const you = youOf(vevent, owner);
    if (you === "declined") return;
    const allDay = start.isDate;
    const from = start.toJSDate();
    const to = end.toJSDate();
    if (!(from < dayEnd && to > dayStart)) return;
    const call = vevent.getFirstPropertyValue("x-google-conference") || CALL.exec(`${vevent.getFirstPropertyValue("location") ?? ""} ${vevent.getFirstPropertyValue("description") ?? ""}`)?.[0];
    events.push({
      title: String(event.summary || "(No title)"),
      allDay,
      ...(allDay ? {} : { start: from.toISOString(), end: to.toISOString() }),
      you,
      ...(typeof call === "string" && call.startsWith("https://") && { join: call }),
      ...(linkOf(event.uid, owner, original.toJSDate(), allDay, recurring) && { link: linkOf(event.uid, owner, original.toJSDate(), allDay, recurring) }),
      ...(owner && idOf(event.uid, original.toJSDate(), allDay, recurring) && { id: idOf(event.uid, original.toJSDate(), allDay, recurring), calendar: owner }),
      refs: refsFrom(`${vevent.getFirstPropertyValue("description") ?? ""} ${vevent.getFirstPropertyValue("location") ?? ""}`),
    });
  };
  masters.forEach((event) => {
    if (!event.isRecurring()) return add(event, event.startDate, event.endDate, Boolean(event.recurrenceId), event.recurrenceId ?? event.startDate);
    const iterator = event.iterator();
    // Occurrences up to the end of today; a long-running series starting
    // years ago is walked quickly, but not forever.
    for (let next = iterator.next(), steps = 0; next && steps < 20_000; next = iterator.next(), steps++) {
      if (next.toJSDate() >= dayEnd) break;
      const occurrence = event.getOccurrenceDetails(next);
      if (occurrence.endDate.toJSDate() > dayStart) add(occurrence.item, occurrence.startDate, occurrence.endDate, true, occurrence.recurrenceId ?? occurrence.startDate);
    }
  });
  return { date: localDate(dayStart), updated: Date.now(), source: "ical", events };
};

// Reads the feed, if there is an address; keeps the last day it read when it
// can't.
const fetchFeed = async (changed = () => {}, log = () => {}) => {
  const url = await secret();
  feed.configured = Boolean(url);
  if (!url) return;
  if (!/^https:\/\/calendar\.google\.com\/calendar\/ical\//.test(url)) {
    feed.error = "The Keychain's google-calendar-ical isn't a Google Calendar iCal address.";
    return changed();
  }
  try {
    const res = await fetch(url, { signal: AbortSignal.timeout(30_000) });
    if (!res.ok) throw new Error(`Google answered ${res.status}`);
    feed.day = dayFrom(await res.text());
    feed.error = undefined;
    feed.fetchedAt = Date.now();
  } catch (err) {
    feed.error = `Couldn't read the calendar feed: ${String(err.message).split("\n")[0]}`;
    log(feed.error);
  }
  changed();
};

module.exports = { FEED_POLL_MS, feed, fetchFeed, dayFrom, idOf, linkOf, refsFrom };
