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
//! new-tab [-s session] [-c dir] [-n name] [-d] [-- command...]
//!                                        prints the new tab's and pane's ids
//! split [-t pane] [-L|-R|-U|-D] [-c dir] [-p percent] [-d] [-- command...]
//!                                        prints the new pane's id
//! send-keys [-t pane] [-l] key...        keys by name (Enter, C-c, M-x), or
//!                                        with -l, text typed as it is
//! capture-pane [-t pane] [-S lines]      the screen (and that much history)
//! select-pane -t pane | select-tab -t tab
//! kill-pane [-t pane] | kill-tab [-t tab]
//! rename-tab [-t tab] name               "" to name it after its directory
//! set [-t target] [-u] @option [value]   a tab's or pane's option
//! show [-t target] @option
//! display [-t pane] pane|tab|session|cwd|command|title
//! list-keys | detach [-s session] | kill-server
//! save                                   the layout now; prints its file
//! say [-t seconds] text...               a message in a box (src/ui)
//! tip <name> | tip seen <name> | tip reset
//!                                        a tip, once; marked shown; all again
//! hook <event> [--parked pids]           from `switchboard hook`, its JSON on
//!                                        stdin (src/agent)
//! ```

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::input::Key;
use super::server::{Reply, Server};
use super::{Direction, PaneId, TabId};

/// The commands the server runs, for the command table: `switchboard
/// <name>` sends them here.
pub const NAMES: &[&str] = &[
    "list",
    "sources",
    "new-tab",
    "split",
    "send-keys",
    "capture-pane",
    "select-pane",
    "select-tab",
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
    "say",
    "tip",
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

/// What a `-t` names.
enum Target {
    Pane(PaneId),
    Tab(TabId),
}

impl Server {
    /// Runs one command; `pane` is the caller's `SWITCHBOARD_PANE`.
    pub fn command(
        &mut self,
        args: &[String],
        cwd: &Path,
        pane: Option<&str>,
        stdin: Option<&[u8]>,
    ) -> Reply {
        let Some((name, rest)) = args.split_first() else {
            return Reply::fail("no command given");
        };
        let caller = pane
            .and_then(|p| p.parse::<PaneId>().ok())
            .filter(|p| self.panes.contains_key(p));
        let result = match name.as_str() {
            "list" => Ok(Reply::ok(format!("{}\n", self.list()))),
            "sources" => Ok(self.sources_command(rest)),
            "hook" => Ok(self.agent_hook(rest, caller, stdin)),
            "new-tab" => self.cmd_new_tab(rest, cwd, caller),
            "split" => self.cmd_split(rest, cwd, caller),
            "send-keys" => self.cmd_send_keys(rest, caller),
            "capture-pane" => self.cmd_capture(rest, caller),
            "select-pane" => self.target_pane(rest, caller).map(|p| {
                self.focus_pane(p);
                Reply::ok("")
            }),
            "select-tab" => self.target_tab(rest, caller).map(|t| {
                if let Some(s) = self.session_of_tab(t).map(|s| s.name.clone()) {
                    self.select_tab(&s, t);
                }
                Reply::ok("")
            }),
            "kill-pane" => self.target_pane(rest, caller).map(|p| {
                self.kill_pane(p);
                Reply::ok("")
            }),
            "kill-tab" => self.target_tab(rest, caller).map(|t| {
                self.kill_tab(t);
                Reply::ok("")
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
                            let _ = c.tx.send(super::proto::ServerMsg::Exit {
                                message: Some("[detached]".into()),
                            });
                        }
                    }
                    Reply::ok("")
                })
            }
            "kill-server" => {
                self.quit = true;
                Ok(Reply::ok(""))
            }
            "say" => Ok(self.cmd_say(rest, caller)),
            "tip" => Ok(self.cmd_tip(rest, caller)),
            "save" => super::persist::save_now(self)
                .map(|file| Reply::ok(format!("{}\n", file.display())))
                .map_err(|e| e.to_string()),
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
        let sessions: Vec<Value> = self
            .sessions
            .iter()
            .map(|s| {
                let tabs: Vec<Value> = s
                    .tabs
                    .iter()
                    .enumerate()
                    .filter_map(|(i, id)| {
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
                                    "cwd": slot.pane.cwd().map(|d| d.to_string_lossy().into_owned()),
                                    "command": slot.pane.foreground_command(),
                                    "title": slot.pane.title(),
                                    "pid": slot.pane.pid(),
                                    "cols": cols,
                                    "rows": rows,
                                    "options": slot.opts,
                                    "agent": self.agent_json(p),
                                }))
                            })
                            .collect();
                        Some(json!({
                            "id": id.to_string(),
                            "index": i + 1,
                            "name": tab.display_name(),
                            "label": self.tab_label(*id),
                            "color": self.tab_color(*id).map(|c| c.hex()),
                            "path": self.tab_path(*id).map(|p| p.to_string_lossy().into_owned()),
                            "pr": self.tab_pr(*id),
                            "facts": self.facts.get(id),
                            "current": *id == s.current,
                            "zoomed": tab.zoomed,
                            "options": tab.opts,
                            "agent": self.tab_agent(*id).map(|a| a.name()),
                            "panes": panes,
                        }))
                    })
                    .collect();
                json!({ "name": s.name, "cols": s.cols, "rows": s.rows, "tabs": tabs })
            })
            .collect();
        let clients: Vec<Value> = self
            .clients
            .values()
            .map(|c| json!({ "session": c.session, "cols": c.cols, "rows": c.rows }))
            .collect();
        json!({ "sessions": sessions, "clients": clients })
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
                a.has('d'),
            )
            .map_err(|e| e.to_string())?;
        Ok(Reply::ok(format!("{tab} {pane}\n")))
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
            .map_err(|e| e.to_string())?;
        Ok(Reply::ok(format!("{pane}\n")))
    }

    fn cmd_send_keys(&mut self, rest: &[String], caller: Option<PaneId>) -> Result<Reply, String> {
        let a = Args::parse(rest, "t")?;
        let pane = self.pane_from(&a, caller)?;
        let slot = self.panes.get_mut(&pane).ok_or("the pane is gone")?;
        let mut bytes = Vec::new();
        for word in &a.rest {
            if a.has('l') {
                bytes.extend_from_slice(word.as_bytes());
            } else {
                match Key::parse(word) {
                    Some(k) => bytes.extend(super::input::encode_key(&k, &slot.pane.modes())),
                    // tmux types a word that isn't a key name as text.
                    None => bytes.extend_from_slice(word.as_bytes()),
                }
            }
        }
        slot.pane.scroll_to_bottom();
        slot.copy = None;
        slot.pane.write(&bytes).map_err(|e| e.to_string())?;
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
        let mut text = slot.pane.text(rows as usize + back);
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
    /// target (`-t @2`), on the tab. Without `-t`, an option whose name
    /// starts `@tab_` or `@agent` (but `@agent_` pane options) goes on the
    /// caller's tab; any other on its pane.
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
                self.dirty_tab(tab);
            }
            Target::Tab(t) => {
                let tab = self.tabs.get_mut(&t).ok_or("the tab is gone")?;
                if unset {
                    tab.opts.remove(name);
                } else {
                    tab.opts.insert(name.clone(), value);
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
        let value = match self.option_target(&a, caller, name)? {
            Target::Pane(p) => self
                .agent_option(p, name)
                .unwrap_or_else(|| self.panes.get(&p).and_then(|s| s.opts.get(name)).cloned()),
            // A tab's `@agent` is its panes' most urgent state.
            Target::Tab(t) if name == "@agent" => self.tab_agent(t).map(|a| a.name().to_string()),
            // What the tab bar shows is worked out by the server (src/tabs).
            Target::Tab(t) if name == "@tab_label" => Some(self.tab_label(t)),
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
        if let Some(t) = a.value('t') {
            return self.parse_target(t);
        }
        let pane = self.default_pane(caller).ok_or("no pane to act on")?;
        let tab_option = name.starts_with("@tab_")
            || (name.starts_with("@agent") && !name.starts_with("@agent_"));
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
            "cwd" => slot
                .pane
                .cwd()
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
fn resolve(cwd: &Path, dir: &str) -> PathBuf {
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
    }

    #[test]
    fn directories_resolve_from_where_the_command_ran() {
        assert_eq!(resolve(Path::new("/a"), "b"), PathBuf::from("/a/b"));
        assert_eq!(resolve(Path::new("/a"), "/c"), PathBuf::from("/c"));
    }
}
