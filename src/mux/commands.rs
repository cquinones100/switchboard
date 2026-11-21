//! What can be asked of the server from a shell, a hook or a skill:
//! `switchboard <command> ...`, sent over the socket and run here on the
//! server's own state. These are what Switchboard's scripts and skills used
//! tmux for (`new-window`, `split-window`, `send-keys`, `capture-pane`,
//! `rename-window`, `set-option`), with a target given as `-t %<pane>` or
//! `-t @<tab>`; without one, the pane the command ran in (its
//! `SWITCHBOARD_PANE`), else the current pane of the client used last.
//!
//! ```text
//! list                                   sessions, tabs and panes, as JSON
//! sources [refresh [view]]               mission control's GitHub and Linear
//!                                        views as JSON, or asked for now
//!                                        (src/sources)
//! new-tab [-s session] [-c dir] [-n name] [-d] [-E] [-- command...]
//!                                        prints the new tab's and pane's ids;
//!                                        -E puts it at the end of the tabs
//! split [-t pane] [-L|-R|-U|-D] [-c dir] [-p percent] [-d] [-- command...]
//!                                        prints the new pane's id
//! respawn-pane [-t pane] [-c dir] [-k] [-- command...]
//!                                        the pane's program replaced, in the
//!                                        same pane; -k ends one still running
//!                                        (src/mux/respawn.rs)
//! send-keys [-t pane] [-l|-p] key...     keys by name (Enter, C-c, M-x), or
//!                                        with -l, text typed as it is, or
//!                                        with -p, text pasted (bracketed
//!                                        when the pane asks), as tmux's
//!                                        paste-buffer -p
//! capture-pane [-t pane] [-S lines] [-D] the screen (and that much history),
//!                                        with -D what is dimmed left blank
//! select-pane [-t pane] [-L|-R|-U|-D]   that pane; with a side, the pane
//!                                        beside it there (the sidebar past
//!                                        the left edge), as an app moving
//!                                        between its own windows hands off
//!                                        at its edges
//! select-tab -t tab
//! switch-client -t tab                   the client used last shown the
//!                                        tab, in its session (tmux's
//!                                        switch-client)
//! kill-pane [-t pane] | kill-tab [-t tab]
//! rename-tab [-t tab] name               "" to name it after its directory
//! set [-t target] [-u] @option [value]   a tab's or pane's option
//! show [-t target] @option
//! display [-t pane] pane|tab|session|cwd|command|title
//! list-keys | detach [-s session] | kill-server
//! save                                   the layout now; prints its file
//! upgrade [--latest]                     the server restarted on the build
//!                                        installed now, keeping every pane
//!                                        (src/mux/upgrade.rs); --latest
//!                                        builds the latest main first, as
//!                                        M-p's restart does, and waits
//!                                        for it
//! say [-t seconds] text...               a message in a box (src/ui)
//! picture [-t pane] <file>               the picture shown full size over
//!                                        the pane, Esc going back; fails
//!                                        where the terminal draws no images
//!                                        (src/ui/picture.rs)
//! look [-t pane] <file>...               pictures shown beside the tab's
//!                                        narrow lone pane, stacked when
//!                                        several; in the desktop's viewer
//!                                        with no room there, and a video
//!                                        or a PDF always (src/ui/pictures.rs)
//! seen [-t tab] [-s from]                the tab seen, as if looked at: its
//!                                        finished Claude and a reminder come
//!                                        due (the Slack relay's Open); -s
//!                                        names who asked (src/phone)
//! prompt [-t pane]                       the prompt a Claude pane waits on,
//!                                        as JSON, null with none (src/phone)
//! answer [-t pane] [-m] [-h id] [-s from] reply
//!                                        the prompt (the one -h names) answered as mission
//!                                        control answers it, or with -m a
//!                                        message typed to its Claude; never
//!                                        typed in a pane (src/phone)
//! remind [-t pane] [-s from] [-n note] (-c | when)
//!                                        a Claude pane's tab's reminder set
//!                                        for when (as the reminder box reads
//!                                        it) or cleared (src/phone)
//! watch                                  every Claude tab's state and last
//!                                        turn, a JSON line each time they
//!                                        change, until it hangs up
//! phone-pair <device>                    a box asking for the code the
//!                                        phone shows, which pairs it
//! tip <name> | tip seen <name> | tip reset
//!                                        a tip, once; marked shown; all again
//! hook <event> [--parked pids]           from `switchboard hook`, its JSON on
//!                                        stdin (src/agent)
//! press <action> [arg]                   an action run as its key would be,
//!                                        by its name in keys.json; the notes
//!                                        app's palette asks this, and its
//!                                        `gw` presses `work-on-item <note>
//!                                        <line>` (src/notes/serve.rs)
//! ```

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::input::Key;
use super::server::{Opened, Reply, Server};
use super::{Direction, PaneId, TabId};

/// The commands the server runs, for the command table: `switchboard
/// <name>` sends them here.
pub const NAMES: &[&str] = &[
    "list",
    "sources",
    "new-tab",
    "split",
    "respawn-pane",
    "send-keys",
    "capture-pane",
    "select-pane",
    "select-tab",
    "switch-client",
    "kill-pane",
    "kill-tab",
    "rename-tab",
    "set",
    "show",
    "display",
    "list-keys",
    "detach",
    "kill-server",
    "save",
    "restore-layout",
    "upgrade",
    "say",
    "picture",
    "look",
    "seen",
    "prompt",
    "answer",
    "remind",
    "watch",
    "phone-pair",
    "ready",
    "tip",
    "press",
    "sidebar-repo",
    "server-version",
    "autopilot-next",
    "autopilot-start",
    "autopilot-board",
    "autopilot-pause",
    "slot-report",
];

/// How each command is used, which `--help` (or `-h`) after its name prints.
pub const USAGE: &[(&str, &str)] = &[
    ("list", "list: sessions, tabs and panes, as JSON"),
    (
        "sources",
        "sources [refresh [view]]: mission control's GitHub and Linear views and today's calendar as JSON, or asked for now",
    ),
    (
        "new-tab",
        "new-tab [-s session] [-c dir] [-n name] [-d] [-E] [-- command...]: prints the new tab's and pane's ids; -d leaves it in the background, -E puts it at the end of the tabs",
    ),
    (
        "split",
        "split [-t pane] [-L|-R|-U|-D] [-c dir] [-p percent] [-d] [-- command...]: prints the new pane's id",
    ),
    (
        "respawn-pane",
        "respawn-pane [-t pane] [-c dir] [-k] [-- command...]: the pane's program replaced, in the same pane; -k ends one still running",
    ),
    (
        "send-keys",
        "send-keys [-t pane] [-l|-p] key...: keys by name (Enter, C-c, M-x), or with -l, text typed as it is, or with -p, text pasted",
    ),
    (
        "capture-pane",
        "capture-pane [-t pane] [-S lines] [-D]: the screen, and that much history; -D leaves what is dimmed blank",
    ),
    (
        "select-pane",
        "select-pane [-t pane] [-L|-R|-U|-D]: that pane; with a side, the pane beside it there",
    ),
    ("select-tab", "select-tab -t tab"),
    (
        "switch-client",
        "switch-client -t tab: the terminal used last shown the tab, in its session",
    ),
    ("kill-pane", "kill-pane [-t pane]"),
    (
        "kill-tab",
        "kill-tab [-t tab] [-s from]: -s names who asked (phone), closing only a tab whose pane runs Claude, logged",
    ),
    (
        "rename-tab",
        "rename-tab [-t tab] name: \"\" to name it after its directory",
    ),
    (
        "set",
        "set [-t target] [-u] @option [value]: a tab's or pane's option",
    ),
    ("show", "show [-t target] @option"),
    (
        "display",
        "display [-t pane] pane|tab|session|cwd|command|title",
    ),
    ("list-keys", "list-keys: every key and its action"),
    (
        "detach",
        "detach [-s session]: the terminals attached (to that session)",
    ),
    (
        "kill-server",
        "kill-server: ends the server and everything in it",
    ),
    ("save", "save: the layout now; prints its file"),
    (
        "restore-layout",
        "restore-layout [<file>]: brings back the tabs of the layout from before the last start (layout.json.start), or of that file (its panes as fresh shells, nothing typed or shown), beside the workspace's one tab",
    ),
    (
        "upgrade",
        "upgrade [--latest]: restarts the server on the build installed now, keeping every pane and its program running; --latest first does what M-p's \"Restart Switchboard on the latest main\" does (brings the main clone up to date, builds it and puts the build in place), waits for it and prints what happened, exiting 1 when it didn't build or restart; only on a build made from Switchboard's clone",
    ),
    ("say", "say [-t seconds] text...: a message in a box"),
    (
        "picture",
        "picture [-t pane] <file>: the picture shown full size over the pane, Esc going back; fails where the terminal does not draw the server's images",
    ),
    (
        "look",
        "look [-t pane] [--before-after <before> <after> | <file>...]: pictures shown in the side panel beside the tab's narrow lone pane, stacked when there are several, the two --before-after takes headed Before and after, M-Esc or a click closing them; opened with the desktop's viewer where there is no room beside it, and a video or a PDF always",
    ),
    (
        "seen",
        "seen [-t tab] [-s from]: the tab seen, as if looked at: its finished Claude and a reminder come due; -s names who asked (phone), written in the server's log",
    ),
    (
        "prompt",
        "prompt [-t pane]: the prompt the pane's Claude waits on, as JSON (its question, options, whether it takes several, its free option), null with none; refused for a pane not running Claude",
    ),
    (
        "answer",
        "answer [-t pane] [-m] [-h id] [-s from] <reply>: the prompt the pane's Claude shows answered as mission control's answer box answers it (an option's number or label, submit, or your own words), refused with -h when it isn't the prompt that id names (prompt's id), or with -m a message typed to it while it shows none; -s names who asked (phone), written in the server's log; refused for a pane not running Claude, and typed in a pane",
    ),
    (
        "remind",
        "remind [-t pane] [-s from] [-n note] (-c | <when>): the reminder of the tab of a pane running Claude set for <when> (one of the reminder box's times, or one typed: 45m, 3pm, tue 9am, nov 20) with -n's note, or cleared with -c, as the box sets it; refused with Settings' Reminders off, for a pane not running Claude, and for a time it can't read; -s names who asked (phone), written in the server's log",
    ),
    (
        "watch",
        "watch: every Claude tab's state, last turn and waiting prompt, a line of JSON now and each time they change, until the reader hangs up",
    ),
    (
        "phone-pair",
        "phone-pair <device>: from switchboard phone, a box asking you to type the code the phone app shows on <device>, which pairs it; Esc refuses it",
    ),
    (
        "ready",
        "ready [-t tab] <what>: what the tab's pane was loading is ready (\"Diff\"): the tab under Needs you, with a notification, while it isn't in front",
    ),
    (
        "tip",
        "tip <name> | tip seen <name> | tip reset: a tip, once; marked shown; all again",
    ),
    (
        "hook",
        "hook <event> [--parked pids]: from switchboard hook, its JSON on stdin",
    ),
    (
        "context",
        "context [<percent> [model]]: from switchboard statusline, that the calling pane's Claude drew its statusline, and how full its context is once known",
    ),
    (
        "press",
        "press <action> [arg]: an action run as its key would be, by its name in keys.json (a resize moves the pane's divider, as its key does outside the sidebar)",
    ),
    (
        "sidebar-repo",
        "sidebar-repo [-s session] <dir> | --all: the sidebar filtered to the repository dir is in (its main clone), every other one's tabs under Other repos; --all clears it",
    ),
    (
        "server-version",
        "server-version: the build the server runs, as switchboard --version names it",
    ),
    (
        "autopilot-next",
        "autopilot-next <note> <line>: the unticked item on that line of the note autopilot works put next in its queue",
    ),
    (
        "autopilot-start",
        "autopilot-start [--change] <fields>: autopilot, while off, started with the settings given as JSON, as its settings box starts it, or with --change, while on, its settings changed as the box's Save changes them; the Slack relay's Autopilot form, refused run by hand",
    ),
    (
        "autopilot-board",
        "autopilot-board: autopilot's progress as JSON, for the Slack relay's Home tab: paused or not, its spend, when it should be done, how many items stand in each column of its board, and its tabs in flight",
    ),
    (
        "autopilot-pause",
        "autopilot-pause on|off: autopilot paused or resumed, as its menu's Pause and Resume do",
    ),
    (
        "slot-report",
        "slot-report wait|hold <kind> <pid> [index cap child] -- command | done <pid>: from the cargo, npm and pnpm stand-ins, a command in the calling pane waiting for, holding or letting go a slot in the queue for heavy work",
    ),
];

/// The commands that read their stdin (a Claude Code hook's JSON); no other
/// command's stdin is read, since one run with a pipe nobody closes would
/// wait for it forever.
pub const TAKES_STDIN: &[&str] = &["hook"];

/// A command's arguments: its flags, with or without a value, and the rest.
struct Args {
    flags: Vec<(char, Option<String>)>,
    rest: Vec<String>,
}

impl Args {
    /// Reads `-x` flags (those in `with_value` take the next argument) up to
    /// the first argument that isn't one, or `--`.
    fn parse(args: &[String], with_value: &str) -> Result<Args, String> {
        let mut flags = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a == "--" {
                i += 1;
                break;
            }
            // A long option first is none of these commands', so it is a
            // mistake rather than a program to run or a key to send; after
            // a flag (send-keys -l --text) it is what is typed.
            if i == 0 && a.starts_with("--") {
                return Err(format!("unknown option {a}"));
            }
            let Some(letters) = a
                .strip_prefix('-')
                .filter(|l| !l.is_empty() && !l.starts_with('-'))
            else {
                break;
            };
            let mut chars = letters.chars();
            while let Some(f) = chars.next() {
                if with_value.contains(f) {
                    let rest: String = chars.collect();
                    let value = if !rest.is_empty() {
                        rest
                    } else {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or_else(|| format!("-{f} needs a value"))?
                    };
                    flags.push((f, Some(value)));
                    break;
                }
                flags.push((f, None));
            }
            i += 1;
        }
        Ok(Args {
            flags,
            rest: args[i.min(args.len())..].to_vec(),
        })
    }

    fn has(&self, f: char) -> bool {
        self.flags.iter().any(|(g, _)| *g == f)
    }

    fn value(&self, f: char) -> Option<&str> {
        self.flags
            .iter()
            .rev()
            .find(|(g, _)| *g == f)
            .and_then(|(_, v)| v.as_deref())
    }
}

/// Whether an option is one of Switchboard's own marks, which a command run
/// by hand in a pane may not set or unset: a tab's `@land` (it lands
/// without asking) and `@land_work` (it follows the setting), autopilot's (`@autopilot_handsfree` answers its
/// questions), but `@autopilot_pushed` and `@autopilot_merged`, which
/// queue-and-clean sets once it has pushed or its PR has merged, and what a pane's program may do (`respawn::PROGRAM_MARKS`:
/// `@allow_passthrough` and `@notes_pane` pass its graphics commands, which
/// can have the terminal read any file, to the terminal).
pub fn own_mark(option: &str) -> bool {
    super::respawn::PROGRAM_MARKS.contains(&option)
        || option == super::server::LAND
        || option == super::server::LAND_WORK
        || (option.starts_with("@autopilot")
            && option != "@autopilot_pushed"
            && option != "@autopilot_merged")
}

/// What a `-t` names.
enum Target {
    Pane(PaneId),
    Tab(TabId),
}

impl Server {
    /// Runs one command; `pane` is the caller's `SWITCHBOARD_PANE`, and
    /// `by_hand` whether it was run as `switchboard <command>` from a
    /// command line rather than asked by one of Switchboard's programs.
    pub fn command(
        &mut self,
        args: &[String],
        cwd: &Path,
        pane: Option<&str>,
        stdin: Option<&[u8]>,
        by_hand: bool,
    ) -> Reply {
        let Some((name, rest)) = args.split_first() else {
            return Reply::fail("no command given");
        };
        if matches!(rest.first().map(String::as_str), Some("--help" | "-h"))
            && let Some((_, usage)) = USAGE.iter().find(|(n, _)| n == name)
        {
            return Reply::ok(format!("usage: switchboard {usage}\n"));
        }
        let caller = pane
            .and_then(|p| p.parse::<PaneId>().ok())
            .filter(|p| self.panes.contains_key(p));
        // `respawn-program` is asked only by `switchboard respawn`, which is
        // run by hand, whatever program asks it.
        let typed_in_a_pane = pane.is_some() && (by_hand || name == "respawn-program");
        if typed_in_a_pane && let Err(refusal) = self.refuse_by_hand(name, rest, caller) {
            self.say(None, &refusal);
            return Reply::fail(refusal);
        }
        let result = match name.as_str() {
            "list" => Ok(Reply::ok(format!("{}\n", self.list()))),
            "sources" => Ok(self.sources_command(rest)),
            "hook" => {
                // What the sidebar read of Claude's files is read again
                // before the next draw (src/sidebar).
                self.sidebar.stale = true;
                Ok(self.agent_hook(rest, caller, stdin))
            }
            "context" => Ok(self.agent_context(rest, caller)),
            "slot-report" => Ok(self.cmd_slot_report(rest, caller)),
            "new-tab" => self.cmd_new_tab(rest, cwd, caller),
            "split" => self.cmd_split(rest, cwd, caller),
            "respawn-pane" => self.cmd_respawn(rest, cwd, caller),
            "respawn-program" => self.cmd_respawn_program(rest, cwd, caller),
            "send-keys" => self.cmd_send_keys(rest, caller),
            "capture-pane" => self.cmd_capture(rest, caller),
            "select-pane" => self.cmd_select_pane(rest, caller),
            "select-tab" => self.target_tab(rest, caller).map(|t| {
                if let Some(s) = self.session_of_tab(t).map(|s| s.name.clone()) {
                    self.select_tab(&s, t);
                }
                Reply::ok("")
            }),
            // tmux's `switch-client` with no `-c`: the client used last
            // among those showing the caller's session (else any), moved to
            // the session holding the tab, and the tab made current there.
            "switch-client" => self.target_tab(rest, caller).map(|t| {
                if let Some(s) = self.session_of_tab(t).map(|s| s.name.clone()) {
                    self.select_tab(&s, t);
                    let here = caller
                        .and_then(|p| self.panes.get(&p))
                        .and_then(|slot| self.session_of_tab(slot.tab))
                        .map(|s| s.name.clone());
                    let client = self
                        .clients
                        .values()
                        .filter(|c| here.as_ref().is_none_or(|h| *h == c.session))
                        .max_by_key(|c| c.activity)
                        .map(|c| c.id)
                        .or_else(|| self.latest_client());
                    if let Some(client) = client {
                        self.switch_client(client, &s);
                    }
                }
                Reply::ok("")
            }),
            "kill-pane" => self.target_pane(rest, caller).map(|p| {
                self.kill_pane(p);
                Reply::ok("")
            }),
            "kill-tab" => Args::parse(rest, "ts").and_then(|a| match a.value('s') {
                Some(from) => {
                    let pane = self.pane_from(&a, caller)?;
                    self.phone_close(pane, from)
                }
                None => {
                    let tab = self.tab_from(&a, caller)?;
                    self.kill_tab(tab);
                    Ok(Reply::ok(""))
                }
            }),
            "rename-tab" => self.cmd_rename(rest, caller),
            "set" => self.cmd_set(rest, caller),
            "show" => self.cmd_show(rest, caller),
            "display" => self.cmd_display(rest, caller),
            "list-keys" => Ok(Reply::ok(
                self.bindings
                    .list()
                    .into_iter()
                    .map(|(k, a)| format!("{k}\t{a}\n"))
                    .collect::<String>(),
            )),
            "detach" => {
                let a = Args::parse(rest, "s").map_err(|e| e.to_string());
                a.map(|a| {
                    let session = a.value('s').map(str::to_string);
                    let gone: Vec<_> = self
                        .clients
                        .values()
                        .filter(|c| session.as_ref().is_none_or(|s| *s == c.session))
                        .map(|c| c.id)
                        .collect();
                    for id in gone {
                        if let Some(c) = self.clients.remove(&id) {
                            c.exit("[detached]".into());
                            self.client_left(&c.session);
                        }
                    }
                    Reply::ok("")
                })
            }
            "server-version" => Ok(Reply::ok(format!("{}\n", crate::update::version_line()))),
            "kill-server" => {
                super::persist::ended_by_hand(self);
                self.quit = true;
                Ok(Reply::ok(""))
            }
            "say" => Ok(self.cmd_say(rest, caller)),
            "picture" => Ok(self.cmd_picture(rest, caller)),
            "look" => Ok(self.cmd_look(rest, cwd, caller)),
            "seen" => Args::parse(rest, "ts").and_then(|a| {
                let tab = self.tab_from(&a, caller)?;
                self.mark_seen(tab);
                if let Some(from) = a.value('s') {
                    self.phone_did(from, tab, "Marked seen");
                }
                Ok(Reply::ok(""))
            }),
            "prompt" => Args::parse(rest, "t")
                .and_then(|a| self.pane_from(&a, caller))
                .and_then(|pane| self.phone_prompt(pane)),
            "answer" => Args::parse(rest, "tsh").and_then(|a| {
                let pane = self.pane_from(&a, caller)?;
                let reply = a.rest.join(" ");
                self.phone_answer(pane, &reply, a.has('m'), a.value('h'), a.value('s'))
            }),
            "remind" => Args::parse(rest, "tsn").and_then(|a| {
                let pane = self.pane_from(&a, caller)?;
                let when = a.rest.join(" ");
                let when = match (a.has('c'), when.trim().is_empty()) {
                    (true, true) => None,
                    (false, false) => Some(when),
                    (true, false) => return Err("remind takes -c or a time, not both".into()),
                    (false, true) => return Err("remind takes a time, or -c to clear it".into()),
                };
                self.phone_remind(
                    pane,
                    when.as_deref(),
                    a.value('n').unwrap_or(""),
                    a.value('s'),
                )
            }),
            // Asked alone, it is answered by `Server::handle` keeping its
            // connection.
            "watch" => Err("watch takes nothing".into()),
            "phone-pair" => match rest.first() {
                Some(device) => self.phone_pair_ask(device),
                None => Err("phone-pair takes the device asking".into()),
            },
            "ready" => Args::parse(rest, "t").and_then(|a| {
                let tab = self.tab_from(&a, caller)?;
                let what = a.rest.join(" ");
                if what.trim().is_empty() {
                    return Err("ready takes what is ready (Diff)".into());
                }
                self.tab_ready(tab, what.trim());
                Ok(Reply::ok(""))
            }),
            "tip" => Ok(self.cmd_tip(rest, caller)),
            "press" => Ok(self.cmd_press(rest, caller)),
            "autopilot-next" => {
                let (Some(note), Some(line)) = (
                    rest.first().filter(|n| crate::notes::serve::in_notes(n)),
                    rest.get(1).and_then(|l| l.parse::<usize>().ok()),
                ) else {
                    return Reply::fail("autopilot-next takes a note and a line number");
                };
                let Some(client) = self.press_client(caller) else {
                    return Reply::fail("autopilot-next: no client is attached to say it in");
                };
                self.autopilot_put_next(client, note.clone(), line, None);
                Ok(Reply::ok(""))
            }
            "autopilot-board" => Ok(Reply::ok(format!(
                "{}\n",
                crate::util::json_compact(&self.autopilot_board())
            ))),
            "autopilot-pause" => match rest.first().map(String::as_str) {
                Some("on") => self.autopilot_pause(true),
                Some("off") => self.autopilot_pause(false),
                _ => Err("autopilot-pause takes on or off".to_string()),
            }
            .map(|said| Reply::ok(format!("{said}\n"))),
            "sidebar-repo" => self.cmd_sidebar_repo(rest, cwd, caller),
            "save" => super::persist::save_now(self)
                .map(|file| Reply::ok(format!("{}\n", file.display())))
                .map_err(|e| format!("{e:#}")),
            // Taken before it reaches here, by the server's loop.
            // `upgrade` alone is answered by the server's loop, and
            // `--latest` reaches it as `upgrade-latest`.
            "upgrade" => Err("usage: switchboard upgrade [--latest]".to_string()),
            "restore-layout" => Err("restore-layout is answered by the server's loop".to_string()),
            other => Err(format!("no command named {other}")),
        };
        result.unwrap_or_else(Reply::fail)
    }

    fn parse_target(&self, t: &str) -> Result<Target, String> {
        if let Ok(p) = t
            .parse::<PaneId>()
            .map_err(|_| ())
            .and_then(|p| if t.starts_with('%') { Ok(p) } else { Err(()) })
        {
            return if self.panes.contains_key(&p) {
                Ok(Target::Pane(p))
            } else {
                Err(format!("no pane {t}"))
            };
        }
        if let Ok(tab) = t
            .parse::<TabId>()
            .map_err(|_| ())
            .and_then(|x| if t.starts_with('@') { Ok(x) } else { Err(()) })
        {
            return if self.tabs.contains_key(&tab) {
                Ok(Target::Tab(tab))
            } else {
                Err(format!("no tab {t}"))
            };
        }
        Err(format!("{t} isn't a pane (%<n>) or a tab (@<n>)"))
    }

    /// The pane a command acts on: `-t`'s, the caller's, or the current one.
    fn default_pane(&self, caller: Option<PaneId>) -> Option<PaneId> {
        caller
            .or_else(|| self.latest_client().and_then(|c| self.current_pane(c)))
            .or_else(|| {
                let s = self.sessions.first()?;
                Some(self.tabs.get(&s.current)?.active)
            })
    }

    fn pane_from(&self, a: &Args, caller: Option<PaneId>) -> Result<PaneId, String> {
        match a.value('t') {
            Some(t) => match self.parse_target(t)? {
                Target::Pane(p) => Ok(p),
                Target::Tab(tab) => Ok(self.tabs[&tab].active),
            },
            None => self
                .default_pane(caller)
                .ok_or_else(|| "no pane to act on".to_string()),
        }
    }

    fn tab_from(&self, a: &Args, caller: Option<PaneId>) -> Result<TabId, String> {
        match a.value('t') {
            Some(t) => match self.parse_target(t)? {
                Target::Pane(p) => Ok(self.panes[&p].tab),
                Target::Tab(tab) => Ok(tab),
            },
            None => self
                .default_pane(caller)
                .map(|p| self.panes[&p].tab)
                .ok_or_else(|| "no tab to act on".to_string()),
        }
    }

    fn target_pane(&self, rest: &[String], caller: Option<PaneId>) -> Result<PaneId, String> {
        self.pane_from(&Args::parse(rest, "t")?, caller)
    }

    fn target_tab(&self, rest: &[String], caller: Option<PaneId>) -> Result<TabId, String> {
        self.tab_from(&Args::parse(rest, "t")?, caller)
    }

    /// Everything, as JSON: sessions, their tabs in order, each tab's panes.
    pub fn list(&self) -> Value {
        let stages = self.list_stages();
        let sessions: Vec<Value> = self
            .sessions
            .iter()
            .map(|s| {
                let tabs: Vec<Value> = s
                    .tabs
                    .iter()
                    .filter_map(|id| {
                        let tab = self.tabs.get(id)?;
                        let panes: Vec<Value> = tab
                            .layout
                            .panes()
                            .into_iter()
                            .filter_map(|p| {
                                let slot = self.panes.get(&p)?;
                                let (cols, rows) = slot.pane.size();
                                Some(json!({
                                    "id": p.to_string(),
                                    "active": p == tab.active,
                                    "cwd": self.pane_dir(p).map(|d| d.to_string_lossy().into_owned()),
                                    "command": slot.pane.foreground_command(),
                                    "title": slot.pane.title(),
                                    "pid": slot.pane.pid(),
                                    "cols": cols,
                                    "rows": rows,
                                    "options": slot.opts,
                                    "agent": self.agent_json(p),
                                    // Whether it runs Claude, its session
                                    // file written yet or not, as the
                                    // sidebar last read it.
                                    "runs_claude": self.sidebar.panes.get(&p).is_some_and(|r| r.claude),
                                    // The Claude session in front on it, as
                                    // the sidebar last read it (the relay's),
                                    // and what another session reaches it by.
                                    "claude": self.sidebar.panes.get(&p).and_then(|r| {
                                        let s = r.session.as_ref()?;
                                        Some(json!({
                                            "session": s.session_id,
                                            "transcript": r.transcript,
                                            // The process in front, not a
                                            // parked job `session` follows.
                                            "peer": {
                                                "session": s.peer_session,
                                                "pid": s.pid,
                                                "name": s.peer_name,
                                                "socket": s.socket,
                                            },
                                        }))
                                    }),
                                }))
                            })
                            .collect();
                        Some(json!({
                            "id": id.to_string(),
                            "index": self.tab_number(*id),
                            "name": tab.display_name(),
                            "own": self.tab_own_name(*id),
                            "label": self.tab_label(*id),
                            "label_without_pr_title": self.tab_label_without_pr_title(*id),
                            "color": self.tab_color(*id).map(|c| c.hex()),
                            "path": self.tab_path(*id).map(|p| p.to_string_lossy().into_owned()),
                            "pr": self.tab_pr(*id),
                            // Every PR it follows, in the order opened
                            // (src/tabs/prs.rs).
                            "prs": self
                                .tab_prs(*id)
                                .iter()
                                .map(crate::tabs::prs::TabPr::key)
                                .collect::<Vec<_>>(),
                            "facts": self.facts.get(id),
                            // Its direct landing, for the Slack relay's
                            // thread header: with the landing queue's facts.
                            "landing": self.tab_landing(*id),
                            "current": *id == s.current,
                            "zoomed": tab.zoomed,
                            "options": tab.opts,
                            "agent": self.tab_agent(*id).map(|a| a.name()),
                            "stage": stages.get(id),
                            "panes": panes,
                        }))
                    })
                    .collect();
                json!({
                    "name": s.name,
                    "cols": s.cols,
                    "rows": s.rows,
                    "sidebar": {
                        "columns": s.sidebar,
                        "focused": self.sidebar.focused(&s.name),
                        // The repository it is filtered to, and Other repos unfolded.
                        "repo": self.sidebar_repo(&s.name),
                        "repo_open": self.sidebar.sides.get(&s.name).is_some_and(|x| x.repo_open),
                        // The repositories it was filtered to, the latest first.
                        "repo_recent": self.sidebar.sides.get(&s.name).map(|x| x.repo_recent.clone()).unwrap_or_default(),
                        // The groups folded, by their stages' keys, and the
                        // notifications.
                        "folded": self.sidebar.sides.get(&s.name).map(|x| x.folded.iter().map(|g| g.key()).collect::<Vec<_>>()).unwrap_or_default(),
                        "notices_folded": self.sidebar.sides.get(&s.name).is_some_and(|x| x.notices_folded),
                    },
                    "tabs": tabs,
                })
            })
            .collect();
        let clients: Vec<Value> = self
            .clients
            .values()
            .map(|c| json!({ "session": c.session, "cols": c.cols, "rows": c.rows }))
            .collect();
        // How long since a key was last pressed in a client, which the Slack
        // relay folds into the desktop's reading as the server's own `away`
        // does (src/presence.rs); since the server started, and `typed`
        // false, until one has been.
        let idle_ms = u64::try_from(self.last_input.elapsed().as_millis()).unwrap_or(u64::MAX);
        json!({
            "sessions": sessions,
            "clients": clients,
            "idle_ms": idle_ms,
            "typed": self.typed,
            // Whether the computer is held awake, and what holds it (src/awake).
            "awake": self.awake_json(),
        })
    }

    /// Each tab's stage as the sidebar groups it once you stop browsing:
    /// its key and title, the stage's place among the sidebar's stages
    /// (`rank`) and the tab's within it (`place`), for the Slack relay's
    /// Home, which groups as the sidebar does. The Notifications window has
    /// none.
    fn list_stages(&self) -> std::collections::HashMap<TabId, Value> {
        use crate::sidebar::model::{self, Rules, Stage};
        let tabs = self.sidebar_all_tabs();
        model::grouped(&tabs, None, &Rules::now())
            .into_iter()
            .flat_map(|g| {
                let rank = Stage::ALL.iter().position(|s| *s == g.stage);
                g.tabs.into_iter().enumerate().map(move |(place, t)| {
                    let stage = json!({
                        "key": g.stage.key(),
                        "title": g.stage.title(),
                        "rank": rank,
                        "place": place,
                    });
                    (t.id, stage)
                })
            })
            .collect()
    }

    /// `sidebar-repo`: the caller's session's sidebar (`-s`'s, else the
    /// one used last) filtered to the repository a directory is in, or
    /// cleared with `--all`.
    fn cmd_sidebar_repo(
        &mut self,
        rest: &[String],
        cwd: &Path,
        caller: Option<PaneId>,
    ) -> Result<Reply, String> {
        if !crate::settings::is_on("sidebar_repo_filter") {
            return Err(
                "the sidebar repository filter is off in Settings (sidebar_repo_filter)".into(),
            );
        }
        // --all is this command's own, which Args takes for a mistake.
        let all = rest.iter().any(|r| r == "--all");
        let rest: Vec<String> = rest.iter().filter(|r| *r != "--all").cloned().collect();
        let a = Args::parse(&rest, "s")?;
        let session = match a.value('s') {
            Some(s) => s.to_string(),
            None => caller
                .and_then(|p| self.session_of_tab(self.panes[&p].tab))
                .or_else(|| {
                    self.latest_client()
                        .and_then(|c| self.session(&self.clients[&c].session))
                })
                .or_else(|| self.sessions.first())
                .map(|s| s.name.clone())
                .ok_or("no session")?,
        };
        if self.session(&session).is_none() {
            return Err(format!("no session {session}"));
        }
        let repo = match a.rest.as_slice() {
            [] if all => None,
            [dir] if !all => {
                let dir = cwd.join(dir);
                let dir = dir
                    .canonicalize()
                    .map_err(|e| format!("{}: {e}", dir.display()))?;
                Some(crate::tabs::git::main_clone(&dir))
            }
            _ => return Err("usage: switchboard sidebar-repo [-s session] <dir> | --all".into()),
        };
        self.sidebar_set_repo(&session, repo);
        Ok(Reply::ok(""))
    }

    fn cmd_new_tab(
        &mut self,
        rest: &[String],
        cwd: &Path,
        caller: Option<PaneId>,
    ) -> Result<Reply, String> {
        let a = Args::parse(rest, "scn")?;
        let session = match a.value('s') {
            Some(s) => s.to_string(),
            None => caller
                .and_then(|p| self.session_of_tab(self.panes[&p].tab))
                .or_else(|| {
                    self.latest_client()
                        .and_then(|c| self.session(&self.clients[&c].session))
                })
                .or_else(|| self.sessions.first())
                .map(|s| s.name.clone())
                .ok_or("no session to open a tab in")?,
        };
        let dir = a
            .value('c')
            .map(|d| resolve(cwd, d))
            .unwrap_or_else(|| cwd.to_path_buf());
        let command = (!a.rest.is_empty()).then(|| a.rest.clone());
        let (tab, pane) = self
            .new_tab(
                &session,
                Some(dir),
                command,
                a.value('n').map(str::to_string),
                if a.has('d') {
                    Opened::Background
                } else {
                    Opened::Front
                },
            )
            .map_err(|e| format!("{e:#}"))?;
        if a.has('E') {
            self.tab_to_end(tab);
        }
        Ok(Reply::ok(format!("{tab} {pane}\n")))
    }

    /// `select-pane`: the pane named, or with `-L`, `-R`, `-U` or `-D` the
    /// one beside it on that side, as `M-Left` and the rest move from the
    /// pane in front: the diff and Notes ask this at their own edges, as
    /// vim-tmux-navigator ran `tmux select-pane -L`.
    fn cmd_select_pane(
        &mut self,
        rest: &[String],
        caller: Option<PaneId>,
    ) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let pane = self.pane_from(&a, caller)?;
        let side = [
            ('L', Direction::Left),
            ('R', Direction::Right),
            ('U', Direction::Up),
            ('D', Direction::Down),
        ]
        .into_iter()
        .find(|(f, _)| a.has(*f))
        .map(|(_, d)| d);
        match side {
            Some(dir) => self.select_pane_toward(pane, dir),
            None => self.focus_pane(pane),
        }
        Ok(Reply::ok(""))
    }

    fn cmd_split(
        &mut self,
        rest: &[String],
        cwd: &Path,
        caller: Option<PaneId>,
    ) -> Result<Reply, String> {
        let a = Args::parse(rest, "tcp")?;
        let target = self.pane_from(&a, caller)?;
        let side = if a.has('L') {
            Direction::Left
        } else if a.has('U') {
            Direction::Up
        } else if a.has('D') {
            Direction::Down
        } else {
            Direction::Right
        };
        let dir = a.value('c').map(|d| resolve(cwd, d));
        let percent = a
            .value('p')
            .map(|p| {
                p.trim_end_matches('%')
                    .parse::<u16>()
                    .map_err(|_| format!("-p {p} isn't a percentage"))
            })
            .transpose()?;
        let command = (!a.rest.is_empty()).then(|| a.rest.clone());
        let pane = self
            .split(target, side, dir, command, percent, a.has('d'))
            .map_err(|e| format!("{e:#}"))?;
        Ok(Reply::ok(format!("{pane}\n")))
    }

    fn cmd_respawn(
        &mut self,
        rest: &[String],
        cwd: &Path,
        caller: Option<PaneId>,
    ) -> Result<Reply, String> {
        let a = Args::parse(rest, "tc")?;
        let pane = self.pane_from(&a, caller)?;
        let dir = a.value('c').map(|d| resolve(cwd, d));
        let command = (!a.rest.is_empty()).then(|| a.rest.clone());
        self.respawn_pane(pane, dir, command, a.has('k'))
            .map_err(|e| format!("{e:#}"))?;
        Ok(Reply::ok(""))
    }

    /// `respawn-program [-t pane] [-c dir] <program> [args...]`, what
    /// `switchboard respawn` asks: the pane's program ended and `program`
    /// started in its place, the worktree commands as their shell functions
    /// (`respawn::command_for`), so the server knows which program it starts
    /// and can hold one asked from a pane to those `respawn::allowed` names.
    fn cmd_respawn_program(
        &mut self,
        rest: &[String],
        cwd: &Path,
        caller: Option<PaneId>,
    ) -> Result<Reply, String> {
        let a = Args::parse(rest, "tc")?;
        let pane = self.pane_from(&a, caller)?;
        let dir = a.value('c').map(|d| resolve(cwd, d));
        let (program, args) = a.rest.split_first().ok_or("respawn: which program?")?;
        let command = crate::cmd::respawn::command_for(program, args);
        self.respawn_pane(pane, dir, Some(command), true)
            .map_err(|e| format!("{e:#}"))?;
        Ok(Reply::ok(""))
    }

    /// What a command run by hand in a pane, by you in a shell or by Claude
    /// through its Bash tool, may not do, since Claude may run `switchboard
    /// set` and `switchboard respawn` without asking: set Switchboard's own
    /// marks (`own_mark`), set any option on another tab but its name, or
    /// start any program but those `respawn::allowed` names, or in another
    /// tab. Switchboard's own programs (Notes, the worktree commands, the
    /// landing lock, the Slack relay) ask without `by_hand` and keep doing
    /// what they need. Arguments it can't read, or a target it can't find,
    /// are left to the command, which fails with the same error and no box,
    /// since a mistyped command is no refusal.
    fn refuse_by_hand(
        &self,
        name: &str,
        rest: &[String],
        caller: Option<PaneId>,
    ) -> Result<(), String> {
        let own_tab = caller.and_then(|p| self.panes.get(&p)).map(|slot| slot.tab);
        let tab_of = |pane: PaneId| self.panes.get(&pane).map(|slot| slot.tab);
        match name {
            // `answer` presses a prompt's option for whoever asks, so it
            // isn't one more way for a Claude to approve what it asked
            // permission for; send-keys typed in a pane still reaches any
            // pane, as before.
            "answer" => Err(
                "switchboard answer is asked by Switchboard's own programs, not typed in a pane"
                    .into(),
            ),
            "set" => {
                let Ok(a) = Args::parse(rest, "t") else {
                    return Ok(());
                };
                let Some(option) = a.rest.first() else {
                    return Ok(());
                };
                if own_mark(option) {
                    return Err(format!(
                        "switchboard set: {option} is Switchboard's own to set, not a command's typed in a pane"
                    ));
                }
                let tab = match self.option_target(&a, caller, option) {
                    Ok(Target::Pane(p)) => tab_of(p),
                    Ok(Target::Tab(t)) => Some(t),
                    Err(_) => return Ok(()),
                };
                if tab != own_tab && option != "@tab_name" {
                    return Err(format!(
                        "switchboard set: a command typed in a pane sets {option} on its own tab only (another tab's @tab_name alone)"
                    ));
                }
                Ok(())
            }
            "respawn-pane" | "respawn-program" => {
                let Ok(a) = Args::parse(rest, "tc") else {
                    return Ok(());
                };
                let Ok(pane) = self.pane_from(&a, caller) else {
                    return Ok(());
                };
                if tab_of(pane) != own_tab {
                    return Err(
                        "switchboard respawn: a command typed in a pane respawns a pane of its own tab only"
                            .into(),
                    );
                }
                let allowed = match a.rest.split_first() {
                    None => true,
                    Some(_) if name == "respawn-pane" => false,
                    Some((program, args)) => crate::cmd::respawn::allowed(program, args),
                };
                if !allowed {
                    return Err(format!(
                        "switchboard respawn: a command typed in a pane starts only {}",
                        crate::cmd::respawn::ALLOWED
                    ));
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn cmd_send_keys(&mut self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let pane = self.pane_from(&a, caller)?;
        let slot = self.panes.get_mut(&pane).ok_or("the pane is gone")?;
        let mut bytes = Vec::new();
        for word in &a.rest {
            if a.has('p') {
                bytes.extend(super::input::encode_paste(word, &slot.pane.modes()));
            } else if a.has('l') {
                bytes.extend_from_slice(word.as_bytes());
            } else {
                match Key::parse(word) {
                    Some(k) => bytes.extend(super::input::encode_key(&k, &slot.pane.modes())),
                    // tmux types a word that isn't a key name as text.
                    None => bytes.extend_from_slice(word.as_bytes()),
                }
            }
        }
        slot.pane.copy_end();
        slot.copy = None;
        slot.pane.write(&bytes).map_err(|e| format!("{e:#}"))?;
        Ok(Reply::ok(""))
    }

    fn cmd_capture(&self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "tS")?;
        let pane = self.pane_from(&a, caller)?;
        let slot = self.panes.get(&pane).ok_or("the pane is gone")?;
        let (_, rows) = slot.pane.size();
        let back = match a.value('S') {
            Some(s) => s
                .trim_start_matches('-')
                .parse::<usize>()
                .map_err(|_| format!("-S {s} isn't a number of lines"))?,
            None => 0,
        };
        let lines = rows as usize + back;
        let mut text = if a.has('D') {
            slot.pane.typed_text(lines)
        } else {
            slot.pane.text(lines)
        };
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        Ok(Reply::ok(text))
    }

    fn cmd_rename(&mut self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let tab = self.tab_from(&a, caller)?;
        let name = a.rest.join(" ");
        if let Some(t) = self.tabs.get_mut(&tab) {
            t.name = name;
        }
        self.dirty_tab(tab);
        Ok(Reply::ok(""))
    }

    /// `set -t %3 @agent_state working`: an option on a pane, or with a tab
    /// target (`-t @2`), on the tab. An option whose name starts `@tab_`,
    /// `@autopilot` or `@agent` (but `@agent_` pane options) goes on a tab: the one named,
    /// else the named pane's, else the caller's; any other on the pane.
    fn cmd_set(&mut self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let Some(name) = a.rest.first() else {
            return Err("set: which option?".into());
        };
        let value = a.rest.get(1..).map(|v| v.join(" ")).unwrap_or_default();
        let unset = a.has('u');
        match self.option_target(&a, caller, name)? {
            Target::Pane(p) => {
                // What Claude is doing is kept by the agent state, typed.
                if let Some(set) = self.set_agent_option(p, name, (!unset).then_some(&value)) {
                    return set.map(|()| Reply::ok(""));
                }
                let slot = self.panes.get_mut(&p).ok_or("the pane is gone")?;
                if unset {
                    slot.opts.remove(name);
                } else {
                    slot.opts.insert(name.clone(), value);
                }
                let tab = slot.tab;
                // The shape its app names over the pane shows at once.
                if name == crate::ui::pointer::PANE_OPTION {
                    self.sync_pointers();
                    return Ok(Reply::ok(""));
                }
                self.dirty_tab(tab);
            }
            Target::Tab(t) => {
                let tab = self.tabs.get_mut(&t).ok_or("the tab is gone")?;
                if unset {
                    tab.opts.remove(name);
                } else {
                    crate::autopilot::landed_at(
                        &mut tab.opts,
                        name,
                        &value,
                        crate::util::now_secs(),
                    );
                    tab.opts.insert(name.clone(), value);
                }
                // A lone pane's own width resizes its program too.
                if name == super::layout::OWN_WIDTH {
                    self.relayout(t);
                }
                // A landing may have moved main past the running build.
                if name == crate::sidebar::model::LANDED && !unset {
                    self.behind_check();
                }
                // A plan, note or ticket set as a new tab opens may be what
                // goes beside its pane.
                if name != crate::tabs::beside::OPTION {
                    self.beside_check(t);
                }
                self.dirty_tab(t);
            }
        }
        Ok(Reply::ok(""))
    }

    fn cmd_show(&self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let Some(name) = a.rest.first() else {
            return Err("show: which option?".into());
        };
        // The whole workspace's, for `switchboard setup`'s report.
        if name == "@statusline_skipped" {
            let skipped = self.statusline_skipped().then_some("1\n");
            return Ok(Reply::ok(skipped.unwrap_or_default()));
        }
        let value = match self.option_target(&a, caller, name)? {
            // What the terminals showing the pane's session get, for an app
            // in it to draw by (src/mux/graphics.rs).
            Target::Pane(p) if name == "@graphics" => Some(self.graphics_for_pane(p)),
            Target::Pane(p) => self
                .agent_option(p, name)
                .unwrap_or_else(|| self.panes.get(&p).and_then(|s| s.opts.get(name)).cloned()),
            // A tab's `@agent` is its panes' most urgent state.
            Target::Tab(t) if name == "@agent" => self.tab_agent(t).map(|a| a.name().to_string()),
            // What the tab bar shows is worked out by the server (src/tabs).
            Target::Tab(t) if name == "@tab_label" => Some(self.tab_label(t)),
            Target::Tab(t) if name == "@tab_label_without_pr_title" => {
                Some(self.tab_label_without_pr_title(t))
            }
            Target::Tab(t) if name == "@tab_age" => self
                .tabs
                .get(&t)
                .and_then(|tab| crate::tabs::age(&tab.opts, crate::util::now_secs())),
            // A tab's own tint as `<hue> <step>`, for the statusline in one
            // answer, and only while it is the one drawn, so the statusline
            // falls back to the directory's otherwise.
            Target::Tab(t) if name == "@tab_tint" => self
                .tabs
                .get(&t)
                .filter(|_| crate::settings::is_on("tab_colors_apart"))
                .and_then(|tab| crate::tabs::color::Tint::of_opts(&tab.opts))
                .map(|tint| format!("{} {}", tint.hue, tint.step.name())),
            Target::Tab(t) if name == "@tab_path" => {
                self.tab_path(t).map(|p| p.to_string_lossy().into_owned())
            }
            Target::Tab(t) => self
                .tabs
                .get(&t)
                .and_then(|tab| tab.opts.get(name))
                .cloned(),
        };
        Ok(Reply::ok(
            value.map(|v| format!("{v}\n")).unwrap_or_default(),
        ))
    }

    fn option_target(
        &self,
        a: &Args,
        caller: Option<PaneId>,
        name: &str,
    ) -> Result<Target, String> {
        // Autopilot's marks (src/autopilot) are its tab's too, so
        // a skill in one of its tabs reads `@autopilot_handsfree` as the tab
        // has it, a tool standing in for Claude's pane or not; `@land` is the
        // tab's own the same way.
        let tab_option = name.starts_with("@tab_")
            || name == super::server::LAND
            || name == super::server::LAND_WORK
            || name.starts_with("@autopilot")
            || (name.starts_with("@agent") && !name.starts_with("@agent_"))
            || name == super::layout::OWN_WIDTH;
        // A pane named for a tab's option means its tab, as without `-t`.
        if let Some(t) = a.value('t') {
            return match self.parse_target(t)? {
                Target::Pane(p) if tab_option => self
                    .panes
                    .get(&p)
                    .map(|slot| Target::Tab(slot.tab))
                    .ok_or_else(|| "the pane is gone".to_string()),
                target => Ok(target),
            };
        }
        let pane = self.default_pane(caller).ok_or("no pane to act on")?;
        Ok(if tab_option {
            Target::Tab(self.panes[&pane].tab)
        } else {
            Target::Pane(pane)
        })
    }

    fn cmd_display(&self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let pane = self.pane_from(&a, caller)?;
        let slot = self.panes.get(&pane).ok_or("the pane is gone")?;
        let what = a.rest.first().map(String::as_str).unwrap_or("pane");
        let value = match what {
            "pane" => pane.to_string(),
            "tab" => slot.tab.to_string(),
            "session" => self
                .session_of_tab(slot.tab)
                .map(|s| s.name.clone())
                .unwrap_or_default(),
            "cwd" => self
                .pane_dir(pane)
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_default(),
            "command" => slot.pane.foreground_command().unwrap_or_default(),
            "title" => slot.pane.title().to_string(),
            other => {
                return Err(format!(
                    "display: {other} isn't one of pane, tab, session, cwd, command, title"
                ));
            }
        };
        Ok(Reply::ok(format!("{value}\n")))
    }
}

/// A directory given to a command, relative to where it ran.
pub(crate) fn resolve(cwd: &Path, dir: &str) -> PathBuf {
    let expanded = match dir.strip_prefix("~") {
        Some(rest) if rest.is_empty() || rest.starts_with('/') => {
            crate::paths::home().join(rest.trim_start_matches('/'))
        }
        _ => PathBuf::from(dir),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switchboards_own_marks_are_autopilots_but_the_landings_and_a_programs() {
        for mark in [
            "@land",
            "@land_work",
            "@autopilot_handsfree",
            "@autopilot",
            "@allow_passthrough",
            "@notes_pane",
        ] {
            assert!(own_mark(mark), "{mark}");
        }
        for option in [
            "@autopilot_pushed",
            "@autopilot_merged",
            "@tab_name",
            "@tab_note",
            "@agent_phase",
        ] {
            assert!(!own_mark(option), "{option}");
        }
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn flags_take_values_and_stop_at_the_command() {
        let a = Args::parse(
            &args(&["-d", "-c", "/tmp", "-n", "x", "--", "claude", "-c"]),
            "cn",
        )
        .unwrap();
        assert!(a.has('d'));
        assert_eq!(a.value('c'), Some("/tmp"));
        assert_eq!(a.value('n'), Some("x"));
        assert_eq!(a.rest, args(&["claude", "-c"]));
        let joined = Args::parse(&args(&["-tl", "%3"]), "t").unwrap();
        assert_eq!(
            joined.value('t'),
            Some("l"),
            "a value joined to its flag is taken as its value, as getopt does"
        );
        let b = Args::parse(&args(&["-lt", "%3", "hi"]), "t").unwrap();
        assert!(b.has('l'));
        assert_eq!(b.value('t'), Some("%3"));
        assert_eq!(b.rest, args(&["hi"]));
        assert!(Args::parse(&args(&["-t"]), "t").is_err());
        assert!(
            Args::parse(&args(&["--wat"]), "t").is_err(),
            "a long option first isn't a program to run"
        );
        let typed = Args::parse(&args(&["-l", "--text"]), "t").unwrap();
        assert_eq!(typed.rest, args(&["--text"]), "after a flag it is typed");
    }

    #[test]
    fn every_command_says_how_it_is_used() {
        for name in NAMES {
            assert!(
                USAGE.iter().any(|(n, u)| n == name && u.starts_with(name)),
                "{name} has no usage"
            );
        }
    }

    #[test]
    fn directories_resolve_from_where_the_command_ran() {
        assert_eq!(resolve(Path::new("/a"), "b"), PathBuf::from("/a/b"));
        assert_eq!(resolve(Path::new("/a"), "/c"), PathBuf::from("/c"));
    }
}
