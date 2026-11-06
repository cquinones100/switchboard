---
name: today
description: Show today's Google Calendar events as a table, with times, length, where to join and your reply. Use it whenever you're asked what's on the calendar today, what meetings are left, or what the day looks like.
argument-hint: "[tomorrow | a date]"
---

# Today

This is Switchboard's personal copy, installed for repositories that have none. Before anything else, run `skill-path today`: if it prints a path other than this file's, read that file and follow it instead, with the same arguments, and stop reading here.

Show the user's day from their Google Calendar as one table, through the Google Calendar connector.

Arguments: none for today, or `tomorrow`, or a date. The rest of this says "today" for whichever day was asked for.

## Steps

1. **Check the connector.** Load `list_events` from the Google Calendar connector. If it is missing, or a call says it needs signing in, tell the user to run `/mcp` and sign in to Google Calendar, copy `/mcp` to the clipboard, and stop.

2. **Read the day.** One `list_events` call on the primary calendar, with `startTime` at the day's midnight and `endTime` at the next midnight, both local time with no offset and no `Z` (`2026-10-28T00:00:00`), `orderBy: "startTime"`, and no `eventType`: the default already has regular, out-of-office and focus-time events, and naming them has been refused as over quota. Leave `timeZone` unset, so the server uses the user's own. Follow `nextPageToken` until there is none. Read only the primary calendar: shared team calendars hold everyone's events, not the user's.

3. **Drop what isn't the user's.** Leave out events the user has declined. Keep tentative and unanswered ones; the table says which they are.

   **Save the day for mission control**, only when the day asked for is today: its Calendar view draws what this saves, since it can't reach the calendar itself. Pass the events kept as JSON to the script beside this file:

   ```
   node ~/.claude/skills/today/scripts/save.js <<'EOF'
   {"date": "2026-10-28", "events": [
     {"title": "Standup", "start": "2026-10-28T13:00:00-04:00", "end": "2026-10-28T13:15:00-04:00", "allDay": false, "you": "going", "join": "https://…zoom.us/j/…", "link": "<htmlLink>"}
   ]}
   EOF
   ```

   `start` and `end` as the API gives them (no times and `"allDay": true` for an all-day event), `you` as the table's You column says it, `join` the call's link as the Where column finds it, and `link` the event's `htmlLink`. A failure here doesn't stop the reply.

4. **Show the table.** All-day events first, then the rest by start time:

   | Time | Event | Length | Where | You |
   |---|---|---|---|---|
   | 10:00–10:30 AM | Weekly planning | 30m | [Meet](https://meet.google.com/…) | going |
   | ▶ 11:00 AM–12:00 PM | Design review | 1h | Room 4 | maybe |

   - **Time:** start and end in the user's time zone, 12-hour; "all day" for an all-day event. On today, mark the event happening now with ▶, and leave the ones already over in the table, in place, with their title struck through (`~~Standup~~`).
   - **Event:** the title, linked to the event's page in Google Calendar (`htmlLink`). "Out of office" or "Focus time" for those kinds, when the title doesn't already say so.
   - **Length:** `30m`, `1h`, `1h 30m`.
   - **Where:** the video call as a link named for its service (Meet, Zoom, Teams), from `conferenceData` or a meeting URL in `location`, where Zoom invitations put it; else the location; else `—`. Never copy passcodes or dial-in numbers from the description.
   - **You:** going, maybe, not answered, or "yours" when the user organized it and has no reply of their own.

   Leave out any column that would be empty for every row. Under the table, one line only when it helps: overlapping events (name them), or the next free stretch of an hour or more today. With no events, say the calendar is clear instead of a table.

Reply with the table and that line, nothing else. Never accept, decline, create or change an event here; that is for the user to ask for.
