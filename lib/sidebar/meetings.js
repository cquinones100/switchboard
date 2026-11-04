// Part of bin/tmux-sidebar; see the comment at the top of that file.
//
// Meeting notes: each meeting of yours Fathom recorded becomes a note in the
// notes app, Meetings/<title> <date>.md, with its summary, its action items
// (yours first) and its transcript, its timestamps linking back to the
// recording. Fathom is reached through Claude's connector (lib/connector.js),
// a headless Haiku allowed one Fathom call at a time: one list every
// MEETINGS_POLL_MS during the working day, then a summary and a transcript
// for each new meeting. A meeting is yours when you recorded it or are among
// its invitees, by your Fathom email or your git name. A meeting whose summary
// isn't ready yet is tried again on the next look; one written, or whose note
// is already there, is remembered in STATE_DIR/meetings.json and never written
// again, so the note is yours to edit. Off the drawing path, only while the
// meeting_notes setting is on.

const fs = require("node:fs");
const { noteWritten } = require("./new-notes");
const os = require("node:os");
const path = require("node:path");
const { execFile } = require("node:child_process");
const { STATE_DIR } = require("./config");
const { callConnector } = require("../connector");

const MEETINGS_POLL_MS = 30 * 60_000;
// For the half hour after a meeting on your calendar ends, a look every few
// minutes, since Fathom has its summary a few minutes after the call.
const AFTER_MEETING_MS = 30 * 60_000;
const AFTER_MEETING_POLL_MS = 5 * 60_000;
// Looked for only between these hours, on any day.
const MEETINGS_HOURS = [7, 21];
// The first look goes back this far.
const FIRST_LOOK_DAYS = 2;
const NOTES = () => process.env.NOTES_DIR || path.join(os.homedir(), "Notes");
const STATE = () => path.join(STATE_DIR, "meetings.json");
const FATHOM = "mcp__claude_ai_Fathom__";

// list_meetings' text, a meeting a line, its summary and action items
// indented under it:
//   - Title | 2026-10-28 | id: 1 | url: https://… | recorded by a@b.c | Ann, Bo
//     Action items:
//       • [ ] Do it — assigned to Ann (https://…?timestamp=12.9)
const meetingsFrom = (text) => {
  const meetings = [];
  String(text ?? "")
    .split("\n")
    .forEach((line) => {
      const head = /^- (.*) \| (\d{4}-\d{2}-\d{2}) \| id: (\d+) \| url: (\S+) \| recorded by (.*?)(?: \| (.*))?$/.exec(line);
      if (head) {
        const [, title, date, id, url, recordedBy, invitees = ""] = head;
        return meetings.push({ title, date, id: Number(id), url, recordedBy, invitees: invitees.split(", ").filter(Boolean), actions: [] });
      }
      const action = /^\s+• \[( |x)\] (.*?)(?: — assigned to (.*?))?(?: \((https:\/\/\S+)\))?$/.exec(line);
      if (action && meetings.length) meetings.at(-1).actions.push({ done: action[1] === "x", text: action[2], who: action[3] ?? "", link: action[4] });
    });
  return meetings;
};

const isMine = (meeting, me) =>
  !me.length || [meeting.recordedBy, ...meeting.invitees].some((who) => me.some((m) => who.toLowerCase().includes(m.toLowerCase())));

const timestamp = (link) => {
  const s = Math.floor(Number(/timestamp=([\d.]+)/.exec(link ?? "")?.[1] ?? NaN));
  return Number.isFinite(s) ? `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}` : "";
};

const meetingFile = (meeting) => path.join(NOTES(), "Meetings", `${meeting.title.replace(/[/:\\]/g, "-").replace(/^\.+/, "").trim()} ${meeting.date}.md`);

// The note, from what Fathom gave alone, so its test needs no network.
// Fathom links every line of a summary to its moment in the recording; the
// note keeps the words, and the transcript keeps its timestamps' links.
const meetingNote = (meeting, summary, transcript, me = []) => {
  const plainSummary = String(summary ?? "")
    .replace(/\[((?:[^\[\]]|\[[^\]]*\])*)\]\(https:\/\/fathom\.video\/[^)]*\)/g, "$1")
    .replace(/^## Meeting Purpose\n+/m, "")
    .replace(/^(#{2,3}) /gm, "#$1 ")
    .replace(/^ {2}/gm, "")
    .trim();
  // Yours when the assignee is you by whole name (what follows it in
  // parentheses, their pronouns or email, left out) or by an email Fathom
  // gives with it, in parentheses or not, never by part of a name: a git
  // name of "Ann" isn't Joanne's.
  const mine = (a) => {
    const who = a.who.toLowerCase();
    const name = who.replace(/ \([^)]*\)$/, "").trim();
    // A name only whole; an email also among the words, as Fathom may give one beside a name.
    return me.some((m) => name === m.toLowerCase() || (m.includes("@") && who.split(/[\s<>(),]+/).includes(m.toLowerCase())));
  };
  // Yours first, as checkboxes to tick; everyone else's as a plain list,
  // since they aren't yours to do.
  const actions = [...meeting.actions.filter(mine), ...meeting.actions.filter((a) => !mine(a))].map(
    (a) => `- ${mine(a) ? `[${a.done ? "x" : " "}] ` : ""}${a.text}${a.who ? ` (${a.who.replace(/ \([^)]*\)$/, "")})` : ""}${a.link ? ` [${timestamp(a.link)}](${a.link})` : ""}`,
  );
  return [
    `# ${meeting.title} ${meeting.date}`,
    "",
    `[Recording](${meeting.url}) · recorded by ${meeting.recordedBy}${meeting.invitees.length ? ` · ${meeting.invitees.join(", ")}` : ""}`,
    "",
    "## Action items",
    "",
    ...(actions.length ? actions : ["None."]),
    "",
    "## Summary",
    "",
    plainSummary || "None.",
    "",
    "## Transcript",
    "",
    ...String(transcript ?? "")
      .trim()
      .split("\n")
      .filter(Boolean)
      .flatMap((line) => [line, ""]),
  ].join("\n").trimEnd() + "\n";
};

const readState = () => {
  try {
    return JSON.parse(fs.readFileSync(STATE(), "utf8"));
  } catch {
    return { written: [] };
  }
};

const run = (cmd, args) => new Promise((resolve) => execFile(cmd, args, { encoding: "utf8", timeout: 10_000 }, (err, stdout) => resolve(err ? "" : stdout.trim())));

// `events` is the calendar's day (calendar.js's readDay), for when a meeting
// just ended.
const meetingsDue = (now = new Date(), last = 0, events = []) => {
  const justEnded = events.some((e) => !e.allDay && Date.parse(e.end) <= now && now - Date.parse(e.end) < AFTER_MEETING_MS);
  return now.getHours() >= MEETINGS_HOURS[0] && now.getHours() < MEETINGS_HOURS[1] && now - last >= (justEnded ? AFTER_MEETING_POLL_MS : MEETINGS_POLL_MS);
};

const writeMeetings = async (now = new Date(), log = () => {}, call = callConnector) => {
  const state = readState();
  if (!state.me) {
    const identity = JSON.parse((await call(`${FATHOM}get_identity`, {})) || "{}");
    state.me = [...new Set([identity.email, identity.name, await run("git", ["config", "--global", "user.name"])].filter(Boolean))];
  }
  const since = state.since ?? new Date(now - FIRST_LOOK_DAYS * 86_400_000).toISOString();
  const listed = meetingsFrom(await call(`${FATHOM}list_meetings`, { created_after: since, include_action_items: true, max_pages: 3 }));
  const fresh = listed.filter((m) => isMine(m, state.me) && !state.written.includes(m.id));
  let waiting = false;
  for (const meeting of fresh) {
    const file = meetingFile(meeting);
    if (!fs.existsSync(file)) {
      const summary = await call(`${FATHOM}get_meeting_summary`, { recording_id: meeting.id });
      if (!/\S/.test(summary) || /no summary|not (yet )?available|processing/i.test(summary.slice(0, 200))) {
        waiting = true;
        continue;
      }
      const transcript = await call(`${FATHOM}get_meeting_transcript`, { recording_id: meeting.id, url: meeting.url }, { timeout: 300_000 });
      fs.mkdirSync(path.dirname(file), { recursive: true });
      try {
        fs.writeFileSync(file, meetingNote(meeting, summary, transcript, state.me), { flag: "wx" });
        log(`meetings: wrote ${path.basename(file)}`);
        noteWritten(file, "Fathom");
      } catch (err) {
        if (err.code !== "EEXIST") throw err;
      }
    }
    state.written = [...state.written, meeting.id].slice(-500);
  }
  // Look from now on, less a day for meetings Fathom lists late, unless one
  // is still waiting on its summary.
  if (!waiting) state.since = new Date(now - 86_400_000).toISOString();
  fs.mkdirSync(path.dirname(STATE()), { recursive: true });
  fs.writeFileSync(STATE(), JSON.stringify(state));
};

module.exports = { MEETINGS_POLL_MS, meetingsFrom, isMine, meetingNote, meetingFile, meetingsDue, writeMeetings };
