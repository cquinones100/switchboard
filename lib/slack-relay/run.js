// Part of bin/slack-relay; see the comment at the top of that file.

const path = require("node:path");
const { spawn } = require("node:child_process");
const fs = require("node:fs");
const { LOG_FILE, MAX_LOG_BYTES, EVENTS_FILE, CONFIG_FILE, TAB_POLL_MS } = require("./config");
const { privateFile, log, DONT_RESTART, fail, tryTmux } = require("./util");
const { BOT_TOKEN_SERVICE, APP_TOKEN_SERVICE, keychain, loadOwner, slack, openDm, messageFrom, pressFrom, submissionFrom, socketMode } = require("./slack");
const { catchUp } = require("./read");
const { post, update } = require("./post");
const { loadState, saveState, newestIn } = require("./state");
const { handle } = require("./commands");
const { DASHBOARD_MS, HEARTBEAT_MS, refreshDashboard } = require("./dashboard");
const { setting } = require("../settings");
const { stuckTabs } = require("./stuck");
const { newEvents, followUps } = require("./followups");
const { presence } = require("./presence");
const { pruneFiles } = require("./files");
const { renameHomes } = require("./homes");
const { RENEW_MS, renewOfflineNote, sayBackOnline, cancelOfflineNote } = require("./offline");
const { handlePress } = require("./presses");
const { handleSubmission, forgetScreen, refreshScreens } = require("./screens");
const { welcomeButtons } = require("./buttons");
const { milestones } = require("./milestones");
const { alertTabs } = require("./alerts");
const { claudeTabs } = require("./tabs");
const { life, clock, stopRelay, takeLock, ownerOnly } = require("./lifecycle");
const { deliverOutbox } = require("./outbox");

// Handles what Slack sent or a catch-up found: each message once
// (`handled`), however it arrived, in ts order, one at a time (`handling`). The cursor and threads advance past
// everything seen, the relay's own posts included, before anything is acted
// on, so a failure in one message doesn't replay the rest. `settled` is the
// work queued so far, for the tests to wait on.
const dispatcher = (state) => {
  const handled = new Set();
  let handling = Promise.resolve();
  const processMessages = (messages) => {
    const fresh = messages
      .filter((m) => !handled.has(m.ts))
      .sort((a, b) => Number(a.ts) - Number(b.ts));
    fresh.forEach((m) => handled.add(m.ts));
    // Only a catch-up overlapping live events repeats a message, so recent
    // ones are enough.
    [...handled].slice(0, Math.max(0, handled.size - 500)).forEach((ts) => handled.delete(ts));
    // Advance past everything seen, ours included, before acting, so a
    // failure in one message does not replay the rest.
    fresh.forEach((m) => {
      if (m.thread_ts) newestIn(state, m.thread_ts, m.ts);
      if (m.thread_ts && state.threads[m.thread_ts] !== undefined) {
        if (Number(m.ts) > Number(state.threads[m.thread_ts])) state.threads[m.thread_ts] = m.ts;
      } else if (!m.thread_ts && Number(m.ts) > Number(state.cursor)) {
        state.cursor = m.ts;
      }
    });
    const mine = fresh.filter((m) => !m.byClaude);
    if (mine.length) state.lastActivity = Date.now();
    // Writing from Slack counts as away for a while (see presence).
    if (mine.length) presence.fromSlack().catch((err) => log(`presence: ${err.message}`));
    saveState(state);
    mine.forEach((message) => {
      handling = handling.then(async () => {
        try {
          await handle(state, message);
        } catch (err) {
          log(`failed: ${err.message}`);
          await post(state, `🤖 That failed: ${err.message}`, message.thread_ts ?? undefined).catch(() => {});
        }
        saveState(state);
      });
    });
  };
  return { processMessages, settled: () => handling };
};

// The relay itself, run by supervise as a child process.
const relay = async () => {
  if (!process.env.TMUX) fail("not inside tmux; use `slack-relay` to start it in its own tab", DONT_RESTART);
  // A crash is logged, and the exit tells supervise to start it again.
  process.on("uncaughtException", (err) => fail(`crashed: ${err.stack?.split("\n").slice(0, 2).join(" ") ?? err}`));
  process.on("unhandledRejection", (err) => fail(`crashed: ${err?.stack?.split("\n").slice(0, 2).join(" ") ?? err}`));
  ownerOnly();
  takeLock();
  // After a crash, why, from supervise; empty on a normal start.
  const restarted = process.env.SLACK_RELAY_RESTARTED ?? "";
  // Turn the hooks on: they write events only while this file exists. One
  // left by a crashed relay is kept, so no turn that ended meanwhile is lost.
  const keptEvents = fs.existsSync(EVENTS_FILE);
  if (!keptEvents) privateFile(EVENTS_FILE, "");
  // Mark this window as the relay's, so `slack-relay` finds it whatever the
  // tab ends up named, and the relay never reports on itself.
  if (process.env.TMUX_PANE) tryTmux(["set-option", "-w", "-t", process.env.TMUX_PANE, "@slack_relay", "1"]);

  // The Mac awake while the relay runs, and no longer: -w ends it with this
  // process. Not the displays (no -d): they sleep as usual. On a MacBook with
  // its lid closed that matters, since it stays awake only while an external
  // display is connected, and a display that sleeps stays connected where one
  // switched off does not; so the screens can go dark without the Mac
  // sleeping, which caffeinate cannot prevent (clamshell sleep).
  life.caffeinate = spawn("caffeinate", ["-is", "-w", String(process.pid)], { stdio: "ignore" });

  const state = { threads: {}, alerts: {}, reported: {}, following: {}, homes: {}, latest: {}, ...loadState() };
  // Left by earlier versions: links the parked-job lookup replaced.
  delete state.transcripts;
  delete state.lastPost;
  delete state.ci;
  // And the records reactions answered by, before reactions stopped counting.
  delete state.answerable;
  delete state.reactedTo;
  // When you last wrote, for the quick checks' pace: a fresh start counts as
  // now, so the relay is quick while you check it came up.
  state.lastActivity = Date.now();
  if (!keptEvents || state.eventsOffset === undefined) state.eventsOffset = 0;
  process.on("SIGTERM", () => stopRelay(state));
  // Closing the relay's tab sends SIGHUP; that is a stop too, not a crash.
  // The tab's terminal is gone by then, so writing to it must not crash.
  process.stdout.on("error", () => {});
  process.stderr.on("error", () => {});
  process.on("SIGINT", () => stopRelay(state));
  process.on("SIGHUP", () => stopRelay(state));
  // The app's tokens, and whose relay this is: without them it can't start,
  // and restarting won't help, so it says what to set up.
  slack.botToken = keychain(BOT_TOKEN_SERVICE);
  const appToken = keychain(APP_TOKEN_SERVICE);
  const owner = loadOwner();
  if (!slack.botToken || !appToken) {
    fail(`the Slack app's tokens aren't in the Keychain; store them with: security add-generic-password -U -a "$USER" -s ${BOT_TOKEN_SERVICE} -w (and the same for ${APP_TOKEN_SERVICE})`, DONT_RESTART);
  }
  if (!owner) fail(`no owner in ${CONFIG_FILE}; it should hold { "owner": "<your Slack member ID>" }`, DONT_RESTART);
  // State from before the relay was an app, or for another owner, belongs to
  // another conversation: its threads, alerts and homes are dropped.
  if (!state.dm?.app || state.dm.user !== owner) {
    Object.assign(state, { threads: {}, alerts: {}, homes: {}, following: {}, latest: {}, dashboard: undefined, cursor: undefined });
    state.dm = await openDm(owner);
    saveState(state);
  }
  slack.channel = state.dm.channel;
  slack.owner = owner;
  // The workspace's address, for links to threads; kept with the DM.
  if (!state.dm.url) {
    state.dm.url = await slack.api("auth.test").then((auth) => auth.url).catch(() => undefined);
    saveState(state);
  }
  log(`DM ${state.dm.channel}`);
  // The Home tab is current from the start.
  await refreshDashboard(state, { quick: true }).catch((err) => log(`dashboard failed: ${err.message}`));

  // Start from now: earlier messages were already handled, or were not meant
  // for the relay. Tabs already waiting are reported straight away. After a
  // crash, carry on from where the last run got to instead, so a message sent
  // during the restart is still answered.
  // The app's ID, which the welcome message's Home link needs, comes only with
  // an event from Slack. The first one from the relay's workspace records it,
  // and the welcome message gets its Home button.
  const learnApp = (appId, teamId) => {
    if (!appId || teamId !== state.dm.team || state.dm.appId === appId) return;
    state.dm.appId = appId;
    saveState(state);
    log(`app ${appId}`);
    if (state.welcome) update(state, state.welcome, welcome, welcomeButtons(state.dm)).catch((err) => log(`couldn't add Home to the welcome: ${err.message}`));
  };
  const welcome = welcomeText(restarted);
  const kickoff = await sayListening(state, welcome, restarted);
  state.cursor = restarted && state.cursor ? state.cursor : kickoff;
  // If Slack sent the offline note while the relay was gone, say it's back;
  // then keep the next one pushed ahead (see offline.js).
  const keepNote = () => renewOfflineNote(state).catch((err) => log(`offline note failed: ${err.message}`));
  await sayBackOnline(state).catch((err) => log(`back-online note failed: ${err.message}`));
  await keepNote();
  setInterval(keepNote, RENEW_MS);
  saveState(state);
  log("listening; Ctrl-C or `stop` in the DM to end");

  // Tabs: every TAB_POLL_MS, and whenever a hook writes an event, one pass at
  // a time; a pass asked for while one runs runs right after it.
  let tabsBusy = false;
  let tabsAgain = false;
  const tabTick = async () => {
    if (life.stopping || life.leaving) return;
    if (tabsBusy) {
      tabsAgain = true;
      return;
    }
    tabsBusy = true;
    try {
      // One look at tmux, and at the hooks' events, for all four.
      const tabs = claudeTabs();
      const events = newEvents(state);
      await followUps(state, tabs, events);
      // Looked at again after the follow-ups, which may have just reported a
      // turn whose end changed the tab's state.
      const now = claudeTabs();
      await alertTabs(state, now, events);
      await renameHomes(state, now);
      await milestones(state, tabs);
      await stuckTabs(state, tabs);
      await refreshScreens(state, now);
      await deliverOutbox(state);
    } catch (err) {
      log(`tab check failed: ${err.message}`);
    } finally {
      tabsBusy = false;
    }
    if (tabsAgain) {
      tabsAgain = false;
      tabTick();
    }
  };

  // Slack: each message arrives as an event the moment it is sent. Only the
  // ones you typed, in this DM, get past messageFrom; reactions do nothing. On
  // every connection, what was sent while the relay couldn't hear is caught
  // up; the dispatcher handles each message once however it arrived.
  const { processMessages } = dispatcher(state);
  const socket = socketMode({
    appToken,
    onEvent: (event, teamId, appId) => {
      if (life.stopping || life.leaving) return;
      learnApp(appId, teamId);
      // You opened the app's Home tab: show it as it is now.
      if (event.type === "app_home_opened" && event.user === state.dm.user && event.tab === "home") {
        refreshDashboard(state, { quick: true, opened: true }).catch((err) => log(`Home tab failed: ${err.message}`));
        return;
      }
      const message = messageFrom(state, event, teamId);
      if (message) processMessages([message]);
    },
    // A button you pressed, acted on as the reply it stands for, or a form
    // sent from one of the relay's screens. A screen you closed is no longer
    // kept up to date.
    onInteractive: (payload) => {
      if (life.stopping || life.leaving) return;
      learnApp(payload?.api_app_id, payload?.team?.id ?? payload?.user?.team_id);
      if (payload?.type === "view_closed") return forgetScreen(payload.view?.id);
      const press = pressFrom(state, payload);
      const form = !press && submissionFrom(state, payload);
      if (!press && !form) return;
      state.lastActivity = Date.now();
      presence.fromSlack().catch((err) => log(`presence: ${err.message}`));
      (press ? handlePress(state, press) : handleSubmission(state, form))
        .catch((err) => log(`press failed: ${err.message}`))
        .finally(() => saveState(state));
    },
    onConnect: async () => processMessages(await catchUp(state)),
  });
  life.socket = socket;
  // Whether you're at the computer, for alerts about the tab in front of you.
  presence.watch();
  // Images and videos you sent more than a week ago are deleted, with their stills.
  pruneFiles();
  setInterval(pruneFiles, 60 * 60_000);

  // A heartbeat that notices the Mac has slept: the relay is frozen while
  // it sleeps (caffeinate stops idle sleep on mains power, not standby or a
  // closed lid), so a gap between beats is how long it was out. Everything
  // that judges by how long nothing has happened (stuck tabs) waits a while
  // after waking, since nothing could happen while it slept.
  let beat = Date.now();
  setInterval(() => {
    const now = Date.now();
    if (now - beat > SLEEP_GAP_MS) {
      life.wokeAt = now;
      log(`woke after ${Math.round((now - beat) / 60_000)}m asleep`);
      // Asleep long enough, Slack sent the offline note.
      sayBackOnline(state)
        .then(() => renewOfflineNote(state))
        .catch((err) => log(`offline note failed: ${err.message}`));
    }
    beat = now;
  }, 1_000);
  setInterval(tabTick, TAB_POLL_MS);
  // The same failure every minute (the Home tab turned off, say) is logged once.
  // Looked at every DASHBOARD_MS, or every HEARTBEAT_MS with the poll_fast
  // setting off; published at least every HEARTBEAT_MS for its check-in time.
  let dashboardBusy = false;
  let dashboardError;
  let dashboardLooked = 0;
  let heartbeatAt = 0;
  const dueBy = (at, every) => Date.now() - at >= every - DASHBOARD_MS / 2;
  setInterval(async () => {
    if (dashboardBusy || life.stopping || life.leaving) return;
    if (setting("poll_fast") === false && !dueBy(dashboardLooked, HEARTBEAT_MS)) return;
    dashboardBusy = true;
    dashboardLooked = Date.now();
    // Timed from when it starts, so a slow publish doesn't push the next one past the minute.
    const heartbeat = dueBy(heartbeatAt, HEARTBEAT_MS);
    if (heartbeat) heartbeatAt = Date.now();
    try {
      await refreshDashboard(state, { heartbeat });
      dashboardError = undefined;
    } catch (err) {
      if (heartbeat) heartbeatAt = 0;
      if (err.message !== dashboardError) log(`dashboard failed: ${err.message}`);
      dashboardError = err.message;
    } finally {
      dashboardBusy = false;
    }
  }, DASHBOARD_MS);
  // A hook's event (a prompt taken, a turn ended) is acted on as it lands.
  try {
    fs.watch(EVENTS_FILE, () => tabTick());
  } catch (err) {
    log(`not watching the events file: ${err.message}`);
  }
  tabTick();
};

// Keeps the relay running: runs it as a child process and starts it again
// when it crashes, saying so in the DM (the new run posts that). A crash is
// anything but a clean exit: stop, Ctrl-C and closing the tab end it for good.
// After MAX_CRASHES crashes within CRASH_WINDOW_MS it gives up and posts that
// the relay is down. Waits grow between restarts, so a relay that can't
// reach Slack doesn't spin. It can't speak for itself if it is killed too.
const MAX_CRASHES = 5;
const CRASH_WINDOW_MS = 10 * 60_000;
const RESTART_DELAYS_MS = [2_000, 10_000, 30_000, 60_000, 120_000];

// The reason for the last crash, from the log the relay wrote it to.
const lastError = (since) => {
  try {
    return fs.readFileSync(LOG_FILE, "utf8")
      .split("\n")
      .filter((line) => Date.parse(line.split(" ")[0]) >= since && line.includes(" Error: "))
      .at(-1)
      ?.replace(/^\S+ Error: (crashed: )?/, "")
      .slice(0, 300);
  } catch {
    return undefined;
  }
};

const sayDown = async (why) => {
  const state = loadState();
  if (!state.dm?.app) return;
  slack.botToken ??= keychain(BOT_TOKEN_SERVICE);
  slack.channel = state.dm.channel;
  // This says it's down, so the offline note would only say it again.
  await cancelOfflineNote(state).catch(() => {});
  try {
    await post(
      { ...state, threads: {}, alerts: {} },
      `🤖 **Relay down**\n\nIt crashed ${MAX_CRASHES} times in ten minutes, most recently with: ${why}\n\nIt needs starting again from the computer.`,
    );
  } catch (err) {
    log(`could not say the relay is down: ${err.message}`);
  }
};

const supervise = async () => {
  if (!process.env.TMUX) fail("not inside tmux; use `slack-relay` to start it in its own tab");
  ownerOnly();
  try {
    // Three old logs are kept, about six weeks, so an older problem can still be traced.
    if (fs.statSync(LOG_FILE).size > MAX_LOG_BYTES) {
      [2, 1].forEach((n) => fs.existsSync(`${LOG_FILE}.${n}`) && fs.renameSync(`${LOG_FILE}.${n}`, `${LOG_FILE}.${n + 1}`));
      fs.renameSync(LOG_FILE, `${LOG_FILE}.1`);
    }
  } catch {}
  let child;
  // Closing the tab or a kill ends the relay with it, or, between restarts,
  // ends the wait. Ctrl-C reaches the relay directly, and it stops cleanly.
  ["SIGTERM", "SIGHUP"].forEach((signal) =>
    process.on(signal, () => (child?.exitCode === null && child.signalCode === null ? child.kill(signal) : process.exit(0))),
  );
  process.on("SIGINT", () => {});
  process.exit(await watch({ started: (running) => (child = running) }));
};

const welcomeText = (restarted) =>
  restarted
    ? `🤖 **Relay restarted** after a crash, and still listening.\n\nWhat went wrong: ${restarted}`
    : `🤖 **Relay listening** since ${clock()}\n\nEach tab has its own thread here; reply in it to talk to that tab. **Home** shows every tab at a glance, and Open on one gives its controls; **Help** lists every command.`;

// Says the relay is listening, and returns where reading the DM starts from.
// A start on purpose edits the last welcome back, when it is less than
// WELCOME_REUSE_MS old, rather than posting another, so restarts don't fill
// the DM; reading starts from now, since everything before was handled. A
// first start, an old welcome, or a crash posts a new one: a crash is news.
// A link Slack won't take would fail the whole post, so if it does, the
// welcome goes out without Home rather than not at all.
const WELCOME_REUSE_MS = 24 * 60 * 60_000;
const sayListening = async (state, welcome, restarted) => {
  if (!restarted && state.welcome && Date.now() - Number(state.welcome) * 1000 < WELCOME_REUSE_MS) {
    try {
      await update(state, state.welcome, welcome, welcomeButtons(state.dm));
      return (Date.now() / 1000).toFixed(6);
    } catch (err) {
      log(`couldn't reuse the welcome: ${err.message}`);
    }
  }
  const kickoff = await post(state, welcome, undefined, { buttons: welcomeButtons(state.dm) }).catch((err) => {
    log(`welcome refused with Home: ${err.message}`);
    return post(state, welcome, undefined, { buttons: welcomeButtons({ ...state.dm, appId: undefined }) });
  });
  state.welcome = kickoff;
  return kickoff;
};

// The restart loop: runs `command` (the relay) until it ends on purpose, and
// resolves with the code to exit with. After a crash it waits the next of
// `delays` and starts it again, telling it why (SLACK_RELAY_RESTARTED), and
// after MAX_CRASHES within CRASH_WINDOW_MS it calls `down` with the reason
// and gives up. `started` is told each new child, for signals to reach it.
const watch = async ({ command = [ENTRY, "relay"], delays = RESTART_DELAYS_MS, down = sayDown, started = () => {} } = {}) => {
  const crashes = [];
  let restarted = "";
  for (;;) {
    const since = Date.now();
    const child = spawn(process.execPath, command, {
      stdio: "inherit",
      env: { ...process.env, SLACK_RELAY_RESTARTED: restarted },
    });
    started(child);
    const { code, signal } = await new Promise((resolve) => child.on("exit", (code, signal) => resolve({ code, signal })));
    if (code === 0 || code === DONT_RESTART || ["SIGTERM", "SIGINT", "SIGHUP"].includes(signal)) return code ?? 0;
    const why = lastError(since) ?? (signal ? `killed by ${signal}` : `exit code ${code}`);
    crashes.push(Date.now());
    while (crashes[0] < Date.now() - CRASH_WINDOW_MS) crashes.shift();
    if (crashes.length >= MAX_CRASHES) {
      log(`relay crashed ${crashes.length} times in ten minutes; giving up`);
      await down(why);
      // Nothing will read the hooks' events now.
      try {
        fs.unlinkSync(EVENTS_FILE);
      } catch {}
      return 1;
    }
    const delay = delays[crashes.length - 1];
    log(`relay crashed (${why}); restarting in ${delay / 1000}s`);
    restarted = why;
    await new Promise((resolve) => setTimeout(resolve, delay));
  }
};

// The relay is restarted through the command, not this file.
const ENTRY = path.join(__dirname, "../../bin/slack-relay");

// A gap this long between one-second beats means the Mac was asleep.
const SLEEP_GAP_MS = 30_000;

module.exports = { dispatcher, relay, welcomeText, WELCOME_REUSE_MS, sayListening, MAX_CRASHES, CRASH_WINDOW_MS, RESTART_DELAYS_MS, lastError, sayDown, supervise, watch };
