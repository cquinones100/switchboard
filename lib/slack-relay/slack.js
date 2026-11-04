// Part of bin/slack-relay; see the comment at the top of that file.

const { execFileSync } = require("node:child_process");
const { count } = require("../requests");
const fs = require("node:fs");
const { CONFIG_FILE } = require("./config");
const { log } = require("./util");

// ── Slack ───────────────────────────────────────────────────────────────────

// The relay is a Slack app of its own. It talks to you in the app's DM, as
// its bot, through Slack's Web API, and hears you through Socket Mode: a
// WebSocket the relay opens to Slack, so nothing on this Mac listens for
// connections. Two tokens, both in the macOS Keychain and never in a file:
// the bot token (xoxb-…) for the Web API, and the app-level token (xapp-…,
// scope connections:write) for the socket. Stored with
//   security add-generic-password -U -a "$USER" -s <service> -w
// which asks for the token rather than taking it on the command line.
const BOT_TOKEN_SERVICE = "slack-relay-bot-token";
const APP_TOKEN_SERVICE = "slack-relay-app-token";

const keychain = (service) => {
  try {
    return execFileSync("security", ["find-generic-password", "-a", process.env.USER ?? "", "-s", service, "-w"], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return undefined;
  }
};

// Whose relay this is: your Slack member ID, in CONFIG_FILE as { "owner":
// "U…" }. Only what that member sends, in the app's DM with them, is ever
// acted on (see fromOwner). It comes from the file, never from a message, so
// nothing anyone sends can make them the owner.
const loadOwner = () => {
  try {
    const owner = JSON.parse(fs.readFileSync(CONFIG_FILE, "utf8")).owner;
    return /^[UW][A-Z0-9]+$/.test(owner ?? "") ? owner : undefined;
  } catch {
    return undefined;
  }
};

const API = "https://slack.com/api/";
// A call that hasn't answered in this long has failed.
const API_TIMEOUT_MS = 30_000;
// A file sent or fetched can be a video of up to 200 MB.
const FILE_TIMEOUT_MS = 5 * 60_000;

// One Web API call as the bot, with its arguments form-encoded, which every
// method accepts (objects and arrays go as JSON). Slack's own refusal
// (`ok: false`) is an error naming the method and why; a rate limit is waited
// out as Slack asks, up to three times.
//
// Everything the relay writes goes to the DM: a call naming any other
// conversation is refused here, before it reaches Slack, whatever asked for
// it. `slack.channel` is set once the DM is known.
const apiCall = async (method, params = {}) => {
  const channel = params.channel ?? params.channel_id;
  if (channel !== undefined && channel !== slack.channel) throw new Error(`${method}: slack-relay only uses its DM, not ${channel}`);
  // The Home tab is published only to you.
  if (params.user_id !== undefined && params.user_id !== slack.owner) throw new Error(`${method}: slack-relay only shows its Home tab to its owner`);
  const body = new URLSearchParams(
    Object.entries(params)
      .filter(([, value]) => value !== undefined)
      .map(([key, value]) => [key, typeof value === "object" ? JSON.stringify(value) : String(value)]),
  );
  for (let attempt = 0; ; attempt++) {
    count("Slack", "REST", `relay: ${method}`);
    const response = await fetch(`${slack.base}${method}`, {
      method: "POST",
      headers: { Authorization: `Bearer ${slack.botToken}` },
      body,
      signal: AbortSignal.timeout(API_TIMEOUT_MS),
    });
    if (response.status === 429 && attempt < 3) {
      await new Promise((resolve) => setTimeout(resolve, (Number(response.headers.get("retry-after")) || 1) * 1000));
      continue;
    }
    const reply = await response.json();
    if (!reply.ok) throw new Error(`${method}: ${reply.error}`);
    return reply;
  }
};

// A file's bytes, to the address files.getUploadURLExternal gave for it.
const putFile = async (url, content) => {
  count("Slack", "HTTPS", "relay: file upload");
  const response = await fetch(url, { method: "POST", body: content, signal: AbortSignal.timeout(FILE_TIMEOUT_MS) });
  if (!response.ok) throw new Error(`file upload: HTTP ${response.status}`);
};

// A file Slack keeps, such as an image you attached, which needs the bot's
// token to download.
const getFile = async (url) => {
  count("Slack", "HTTPS", "relay: file download");
  const response = await fetch(url, { headers: { Authorization: `Bearer ${slack.botToken}` }, signal: AbortSignal.timeout(FILE_TIMEOUT_MS) });
  if (!response.ok) throw new Error(`file download: HTTP ${response.status}`);
  return Buffer.from(await response.arrayBuffer());
};

// What the relay calls Slack through. The tests put stand-ins in `api`,
// `put` and `get`, or point `base` at a server of their own, so they never
// reach Slack.
const slack = { api: apiCall, put: putFile, get: getFile, base: API, botToken: undefined, channel: undefined, owner: undefined };

// The app's DM with you, its ID kept with the state. conversations.open
// returns the existing one.
const openDm = async (owner) => {
  const opened = await slack.api("conversations.open", { users: owner });
  const auth = await slack.api("auth.test");
  return { channel: opened.channel.id, user: owner, team: auth.team_id, bot: auth.user_id, url: auth.url, app: true };
};

// ── Hearing you ─────────────────────────────────────────────────────────────

// Whether an event is yours to act on: sent by the owner, in the app's DM
// with them, in the relay's workspace, and not by a bot (the relay's own
// posts come back as events too). Anyone in the workspace can open a DM with
// the app; whatever they send fails this and is only logged, with no reply, so
// the app doesn't even say it is listening.
const fromOwner = (state, event, teamId) =>
  event.user === state.dm.user &&
  event.channel === state.dm.channel &&
  !event.bot_id &&
  (teamId === undefined || teamId === state.dm.team);

// Message events that are something you wrote. Edits, deletions and joins are
// other subtypes and are not acted on; a message with a file attached is.
const MESSAGE_SUBTYPES = [undefined, "file_share", "thread_broadcast"];

// Slack escapes &, < and > in message text, and wraps links and mentions in
// <…>; the text you typed is what goes into a tab.
const plainText = (text) =>
  String(text ?? "")
    .replace(/<((?:https?|mailto):[^|>]+)\|([^>]+)>/g, (_, url, shown) => (shown === url.replace(/^(?:https?:\/\/|mailto:)/, "") ? shown : url))
    .replace(/<((?:https?|mailto):[^>]+)>/g, "$1")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .replace(/&amp;/g, "&");

// Whether you typed a message yourself, in a Slack app. Other apps can post
// as you too (anything holding your user token, the Slack connector your
// Claude sessions use included), and their posts carry your member ID like
// your own, so passing for you is not enough: a message you typed carries the
// client_msg_id Slack's own apps give it, which the Web API doesn't let an
// app set, and no app_id. Without this, a Claude session that read something
// malicious could post here as you, as a reply in a tab's thread, and
// drive that tab.
const typedByYou = (event) => Boolean(event.client_msg_id) && !event.app_id && !event.bot_profile;

// A message event as the dispatcher takes it, or undefined when it isn't one
// to act on. A reply has its thread's ts; a message at the top level has none.
const messageFrom = (state, event, teamId) => {
  if (event.type !== "message" || !MESSAGE_SUBTYPES.includes(event.subtype)) return undefined;
  if (!fromOwner(state, event, teamId) || !typedByYou(event)) {
    if (!event.bot_id) log(`ignored a message from ${event.user ?? "someone"} in ${event.channel}${event.app_id ? ` posted by app ${event.app_id}` : ""}: not typed by the relay's owner in its DM`);
    return undefined;
  }
  const thread = event.thread_ts && event.thread_ts !== event.ts ? event.thread_ts : null;
  // Files you attached, which handle downloads if they are images.
  const files = (event.files ?? []).map((f) => ({ id: f.id, name: f.name, mimetype: f.mimetype, size: f.size, url: f.url_private_download ?? f.url_private }));
  return { ts: event.ts, text: plainText(event.text), byClaude: false, thread_ts: thread, ...(files.length ? { files } : {}) };
};


// A button you pressed, as { value, message } (the message it was on, with
// its ts and thread), or undefined when it isn't one to act on. Only Slack's
// own apps can press a button, on a person's tap: no app can press one as
// you, so a press carries no doubt about who made it. It still has to be
// yours, on a message in the relay's DM, in your workspace.
const pressFrom = (state, payload) => {
  if (payload?.type !== "block_actions") return undefined;
  const action = payload.actions?.[0];
  // A link button (the Home tab's "Thread") only opens its link; Slack tells
  // the app it was pressed, and there is nothing to do.
  if (action?.url) return undefined;
  const channel = payload.container?.channel_id ?? payload.channel?.id;
  const team = payload.team?.id ?? payload.user?.team_id;
  // The Home tab is the owner's alone (it is only published to them), and so
  // is a tab's screen, which only opens from it (see screens.js). The Home
  // tab's buttons refresh it or open a tab's screen, and the screen's act
  // from it.
  const fromView = payload.container?.type === "view" && (payload.view?.type === "home" || payload.view?.type === "modal");
  // Picking a skill in the Skill form's menu, which has no value of its own:
  // the form is drawn again to say what that skill takes (see screens.js).
  const chosen = fromView && payload.view.type === "modal" && payload.view.callback_id === "skill" && action?.action_id === "skill" && action?.type === "static_select";
  // The first check that fails, named in the log, since a refused press of
  // your own is otherwise a mystery.
  const refused = [
    [payload.user?.id !== state.dm.user, "not the relay's owner"],
    [!fromView && channel !== state.dm.channel, `on ${payload.container?.type ?? "something"} in ${channel ?? "no conversation"}, not the DM`],
    [team && team !== state.dm.team, `from workspace ${team}`],
    [!action?.value && !chosen, `a ${action?.type ?? "control"} with no value`],
  ].find(([failed]) => failed);
  if (refused) {
    log(`ignored a button press from ${payload.user?.id ?? "someone"}: ${refused[1]}`);
    return undefined;
  }
  if (chosen) {
    let meta;
    try {
      meta = JSON.parse(payload.view.private_metadata);
    } catch {
      return undefined;
    }
    const value = { a: "skill-chosen", w: meta.w, p: meta.p, from: meta.from, skill: String(action.selected_option?.value ?? ""), args: String(payload.view.state?.values?.args?.args?.value ?? "") };
    return { value, message: undefined, view: { id: payload.view.id, hash: payload.view.hash }, triggerId: payload.trigger_id };
  }
  let value;
  try {
    value = JSON.parse(action.value);
  } catch {
    return undefined;
  }
  if (fromView) {
    const allowed = payload.view.type === "home"
      ? ["open", "refresh", "new", "cleanup", "home-opt", "home-dialog"]
      : ["screen-message", "screen-skill", "screen-opt", "screen-dialog", "screen-progress", "screen-rename", "screen-stop", "screen-close"];
    return allowed.includes(value.a) ? { value, message: undefined, view: { id: payload.view.id }, triggerId: payload.trigger_id } : undefined;
  }
  return { value, triggerId: payload.trigger_id, message: { ts: payload.message?.ts ?? payload.container?.message_ts, thread_ts: payload.message?.thread_ts, text: payload.message?.text ?? "" } };
};

// A form sent from one of the relay's screens (see screens.js), from the owner
// only: Rename's Save, Message's Send, Skill's Run, or New tab's Start. With the tab it was
// opened for, what was typed in each field, and the tab's screen under it.
const FORMS = { rename: ["name"], message: ["text"], skill: ["skill", "typed", "args"], new: ["place", "typed", "prompt"] };
const submissionFrom = (state, payload) => {
  const fields = FORMS[payload?.view?.callback_id];
  if (payload?.type !== "view_submission" || !fields) return undefined;
  const team = payload.team?.id ?? payload.user?.team_id;
  if (payload.user?.id !== state.dm.user || (team && team !== state.dm.team)) {
    log(`ignored a form from ${payload.user?.id ?? "someone"}: not the relay's owner`);
    return undefined;
  }
  let meta;
  try {
    meta = payload.view.callback_id === "new" ? {} : JSON.parse(payload.view.private_metadata);
  } catch {
    return undefined;
  }
  // A text field's value, or a menu's chosen option.
  const values = Object.fromEntries(fields.map((field) => {
    const input = payload.view.state?.values?.[field]?.[field];
    return [field, String(input?.value ?? input?.selected_option?.value ?? "")];
  }));
  return { kind: payload.view.callback_id, meta, values, rootView: payload.view.root_view_id ?? payload.view.previous_view_id };
};

// Socket Mode: Slack sends each event down the socket in an envelope, which
// must be acknowledged within three seconds or Slack sends it again, so each
// is acknowledged before anything else. Slack also asks the app to reconnect
// now and then (a "disconnect" message), and a dropped connection is opened
// again, waiting longer after each failure in a row. `onConnect` runs on every
// (re)connection, to pick up what was sent while the socket was down.
const RECONNECT_DELAYS_MS = [1_000, 2_000, 5_000, 10_000, 30_000];
const socketMode = ({ appToken, onEvent, onInteractive = () => {}, onConnect = () => {} }) => {
  let closed = false;
  let socket;
  let failures = 0;
  const open = async () => {
    if (closed) return;
    try {
      count("Slack", "REST", "relay: apps.connections.open");
      const response = await fetch(`${slack.base}apps.connections.open`, {
        method: "POST",
        headers: { Authorization: `Bearer ${appToken}` },
        signal: AbortSignal.timeout(API_TIMEOUT_MS),
      });
      const reply = await response.json();
      if (!reply.ok) throw new Error(`apps.connections.open: ${reply.error}`);
      count("Slack", "WebSocket", "relay: Socket Mode");
      socket = new WebSocket(reply.url);
    } catch (err) {
      return retry(err.message);
    }
    socket.onmessage = (message) => {
      let envelope;
      try {
        envelope = JSON.parse(message.data);
      } catch {
        return;
      }
      if (envelope.envelope_id) socket.send(JSON.stringify({ envelope_id: envelope.envelope_id }));
      if (envelope.type === "hello") {
        failures = 0;
        log("Slack: connected");
        Promise.resolve(onConnect()).catch((err) => log(`catching up failed: ${err.message}`));
      } else if (envelope.type === "disconnect") {
        socket.close();
      } else if (envelope.type === "events_api" && envelope.payload?.event) {
        onEvent(envelope.payload.event, envelope.payload.team_id, envelope.payload.api_app_id);
      } else if (envelope.type === "interactive") {
        onInteractive(envelope.payload);
      }
    };
    socket.onclose = () => retry("connection closed");
    socket.onerror = () => {};
  };
  const retry = (why) => {
    if (closed) return;
    const delay = RECONNECT_DELAYS_MS[Math.min(failures++, RECONNECT_DELAYS_MS.length - 1)];
    log(`Slack: ${why}; reconnecting in ${delay / 1000}s`);
    setTimeout(open, delay);
  };
  open();
  return {
    close: () => {
      closed = true;
      socket?.close();
    },
  };
};

module.exports = { BOT_TOKEN_SERVICE, APP_TOKEN_SERVICE, keychain, loadOwner, API, API_TIMEOUT_MS, apiCall, putFile, getFile, slack, openDm, fromOwner, typedByYou, MESSAGE_SUBTYPES, plainText, messageFrom, pressFrom, submissionFrom, RECONNECT_DELAYS_MS, socketMode };
