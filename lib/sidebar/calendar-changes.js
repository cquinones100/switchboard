// What changed on today's calendar since you last looked: an event added,
// moved, renamed or gone, for the sidebar's notifications (the
// calendar_changes setting). Today's events as you last saw them are kept in
// calendar-seen.json in the state directory, and looking at the Calendar view
// or dismissing the notification takes the day as it is now. A new day, or
// the first look, starts from the day as it is, with nothing to say.
//
// An event is known by its Google ID from the calendar feed, which a moved
// occurrence keeps; a day /today saved has none, so there it is its title and
// start, and a moved one reads as one gone and one added.

const fs = require("node:fs");
const path = require("node:path");
const { STATE_DIR } = require("./config");

const FILE = () => path.join(STATE_DIR, "calendar-seen.json");
// A time as one instant however it was written (a saved day keeps what
// /today wrote: Z or an offset).
const instant = (at) => (at && !Number.isNaN(Date.parse(at)) ? new Date(at).toISOString() : at ?? null);
// Google's ID first, which holds when Google splits a repeating series;
// the feed's own key where there is no Google ID.
const keyOf = (e) => e.id ?? e.key ?? `${e.title}@${instant(e.start) ?? "all day"}`;
const factsOf = (e) => ({ title: e.title, start: instant(e.start), end: instant(e.end), allDay: Boolean(e.allDay) });
// Where the day came from, as the feed marks its own (source "ical"): the
// feed gives every event an ID, /today's saved day none, so their keys can't
// be compared. Each source keeps a baseline of its own, so one standing in
// for the other a while (the saved day until the daemon's first look at the
// feed) neither reads as every event new nor takes the other's changes as seen.
const sourceOf = (day) => (day.source === "ical" ? "feed" : "saved");
const snapshot = (day) => ({ date: day.date, events: Object.fromEntries((day.events ?? []).map((e) => [keyOf(e), factsOf(e)])) });

const clock = (at) => new Date(at).toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" }).replace(":00", "");

// The changes from `seen` (a snapshot) to `day`, each { key, text }, in the
// day's order, then what is gone.
// An occurrence of a repeating event is keyed with when it was meant to
// start (abc_20261102T140000Z, or uid@time), so a series moved to another
// time keys today's afresh: one gone and one added of the same series are
// that occurrence moved.
// Only a recognized occurrence time comes off, never part of the UID, and a
// key with none (a one-off event) is a series of its own.
const seriesOf = (key) => key.replace(/_\d{8}(T\d{6}Z?)?$/, "").replace(/@\d{4}-\d\d-\d\dT[\d:.]+Z$/, "");
const changesFrom = (seen, day) => {
  const was = { ...(seen.events ?? {}) };
  const now = new Map((day.events ?? []).map((e) => [keyOf(e), factsOf(e)]));
  now.forEach((e, key) => {
    if (was[key]) return;
    if (seriesOf(key) === key) return;
    const old = Object.keys(was).find((k) => !now.has(k) && k !== key && seriesOf(k) !== k && seriesOf(k) === seriesOf(key));
    if (old) {
      was[key] = was[old];
      delete was[old];
    }
  });
  const changes = [];
  now.forEach((e, key) => {
    const before = was[key];
    if (!before) changes.push({ key, text: `${e.title} added${e.allDay ? "" : `, ${clock(e.start)}`}` });
    else if (before.start !== e.start) changes.push({ key, text: `${e.title} moved${e.allDay ? "" : ` to ${clock(e.start)}`}` });
    else if (before.end !== e.end) changes.push({ key, text: `${e.title} now ends${e.allDay ? " on another day" : ` at ${clock(e.end)}`}` });
    else if (before.title !== e.title) changes.push({ key, text: `${before.title} renamed ${e.title}` });
  });
  Object.entries(was).forEach(([key, e]) => {
    if (!now.has(key)) changes.push({ key, text: `${e.title} gone from the calendar` });
  });
  return changes;
};

// { feed, saved }: each source's day as last seen.
const readSeen = () => {
  try {
    const saved = JSON.parse(fs.readFileSync(FILE(), "utf8"));
    return saved && typeof saved === "object" && !saved.events ? saved : {};
  } catch {
    return {};
  }
};

// Takes the day as it is now as seen, for its source.
const calendarSeen = (day) => {
  if (!day?.date) return;
  try {
    fs.mkdirSync(STATE_DIR, { recursive: true });
    fs.writeFileSync(`${FILE()}.tmp`, JSON.stringify({ ...readSeen(), [sourceOf(day)]: snapshot(day) }));
    fs.renameSync(`${FILE()}.tmp`, FILE());
  } catch {}
};

// What changed since you last looked; the first look at a day, from its
// source, says nothing.
const today = (at = new Date()) => `${at.getFullYear()}-${String(at.getMonth() + 1).padStart(2, "0")}-${String(at.getDate()).padStart(2, "0")}`;
const calendarChanges = (day) => {
  // Only today's: a day saved yesterday, still read until today's comes, says nothing.
  if (!day?.date || day.date !== today()) return [];
  const seen = readSeen()[sourceOf(day)];
  if (!seen || seen.date !== day.date) {
    calendarSeen(day);
    return [];
  }
  return changesFrom(seen, day);
};

module.exports = { calendarChanges, calendarSeen, changesFrom };
