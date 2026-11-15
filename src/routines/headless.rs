//! A routine's run: a headless `claude -p` in the routine's directory,
//! opening no tab. Its prompt is added to the system prompt with a line
//! naming the tools it may use, the turn ("Run <name>") goes in on stdin,
//! its model is passed, and its session is resumed between runs. Only the
//! routine's tools are allowed (`--allowedTools`), with none of your
//! Claude settings' own allow rules (`--setting-sources ""`: no user,
//! project or local settings file is read, managed settings still are),
//! and `--permission-mode dontAsk` refuses any other rather than asking, which nobody would see:
//! Claude Code's JSON lists the calls refused (`permission_denials`), and a
//! run with any is a failure, whatever its reply says, naming the tools it
//! needed. Started without Switchboard's plugin (`plugin::headless`), so
//! its hooks speak for no tab; a skill is reached by reading the file
//! `switchboard skill-path <name>` prints, as the starters do.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::routine::Routine;
use crate::{paths, requests};

/// The tools a new routine may use: reading, nothing else.
pub const DEFAULT_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

/// A run going this long (seconds) is ended and recorded as failed.
pub const RUN_LIMIT: i64 = 2 * 3600;

/// What a run said, from Claude Code's JSON.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    /// The session it ran in, which the next run resumes.
    pub session: String,
    /// Its reply.
    pub text: String,
    /// What it cost, in dollars, as Claude Code estimates it.
    pub cost: f64,
    /// Its turn ended in an error.
    pub error: bool,
    /// The tools it tried that weren't allowed, each once.
    pub denied: Vec<String>,
}

/// The system prompt a run is given: the routine's own, then which tools
/// it may use.
pub fn system_prompt(r: &Routine) -> String {
    let tools = if r.tools.is_empty() {
        "none".to_string()
    } else {
        r.tools.join(", ")
    };
    format!(
        "{}\n\nThis is a scheduled run with nobody watching. You may use only these tools: {tools}. Any other is refused and fails the run, so don't try one.",
        r.prompt.trim_end()
    )
}

/// Where a tool's rule names a folder by its usual place under `~`, the
/// place it is here: the notes folder (`$NOTES_DIR`) for `~/Notes/`,
/// the state directory (`$SWITCHBOARD_STATE_DIR`) for
/// `~/.local/state/switchboard/`, as an absolute path (`//` in Claude
/// Code's rules) when it isn't the usual one.
pub fn placed(tool: &str, home: &Path, notes: &Path, state: &Path) -> String {
    for (usual, here) in [("Notes", notes), (".local/state/switchboard", state)] {
        let named = format!("(~/{usual}/");
        let Some(at) = tool.find(&named) else {
            continue;
        };
        if here == home.join(usual) {
            return tool.to_string();
        }
        let shown = here.display().to_string();
        let shown = shown.trim_end_matches('/');
        return format!("{}(/{shown}/{}", &tool[..at], &tool[at + named.len()..]);
    }
    tool.to_string()
}

/// The arguments a run of `r` starts `claude` with; the turn goes on stdin.
pub fn args(r: &Routine) -> Vec<String> {
    args_in(
        r,
        &paths::home(),
        &crate::day::Dirs::here().notes,
        &paths::state_dir(),
    )
}

/// `args`, with the folders a tool's rule may name given.
pub fn args_in(r: &Routine, home: &Path, notes: &Path, state: &Path) -> Vec<String> {
    let mut out: Vec<String> = [
        "-p",
        "--output-format",
        "json",
        "--permission-mode",
        "dontAsk",
        "--setting-sources",
        "",
        "--append-system-prompt",
    ]
    .map(String::from)
    .to_vec();
    out.push(system_prompt(r));
    if !r.model.is_empty() {
        out.extend(["--model".to_string(), r.model.clone()]);
    }
    if !r.resumable().is_empty() {
        out.extend(["--resume".to_string(), r.resumable().to_string()]);
    }
    // Only the built-in tools its rules name are there at all, so one that
    // needs no permission (Read in its directory) is no way past its list;
    // a connector's tools aren't built in, and are allowed by name.
    let mut built_in: Vec<&str> = Vec::new();
    for tool in &r.tools {
        let name = tool.split('(').next().unwrap_or("").trim();
        if !name.is_empty() && !name.starts_with("mcp__") && !built_in.contains(&name) {
            built_in.push(name);
        }
    }
    out.extend(["--tools".to_string(), built_in.join(",")]);
    if !r.tools.is_empty() {
        out.push("--allowedTools".into());
        out.extend(r.tools.iter().map(|t| placed(t, home, notes, state)));
    }
    out
}

/// What Claude Code printed with `--output-format json`, or `None` when it
/// isn't that.
pub fn read_reply(stdout: &str) -> Option<Reply> {
    // The last line holding an object: anything before it is noise.
    let v: Value = stdout
        .lines()
        .rev()
        .find_map(|l| {
            serde_json::from_str::<Value>(l.trim())
                .ok()
                .filter(Value::is_object)
        })
        .or_else(|| serde_json::from_str(stdout.trim()).ok())?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut denied: Vec<String> = Vec::new();
    for d in v
        .get("permission_denials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let tool = d.get("tool_name").and_then(Value::as_str).unwrap_or("");
        if !tool.is_empty() && !denied.iter().any(|t| t == tool) {
            denied.push(tool.to_string());
        }
    }
    let error = v.get("is_error").and_then(Value::as_bool) == Some(true)
        || text("subtype").starts_with("error");
    Some(Reply {
        session: text("session_id"),
        text: text("result"),
        cost: v
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0),
        error,
        denied,
    })
}

/// A reply's first line that says something.
pub fn first_line(text: &str) -> String {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("")
        .to_string()
}

/// A run's files in the routines' folder: what Claude prints
/// (`run-<id>.out`, its JSON, named so as not to be read as a routine),
/// what it says on stderr (`run-<id>.err`), and the
/// run itself (`run-<id>.live`, `Started`), so a server started again,
/// or one an upgrade replaced, takes up a run still going.
pub fn files(dir: &Path, id: &str) -> [PathBuf; 3] {
    ["out", "err", "live"].map(|ext| dir.join(format!("run-{id}.{ext}")))
}

/// A run under way, as its `.live` file keeps it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Started {
    pub pid: u32,
    /// When its process started (milliseconds), which tells it from a
    /// later process given the same id; 0 when it couldn't be read.
    #[serde(default)]
    pub born: u64,
    /// When it started (seconds).
    pub started: i64,
    /// When it was due, and whether it was missed while asleep.
    pub due: i64,
    pub late: bool,
    /// The directory it runs in, where its session is made.
    pub dir: String,
}

/// The runs a server before this one left going, by routine id.
pub fn left_going(dir: &Path) -> Vec<(String, Started)> {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name
                .strip_prefix("run-")?
                .strip_suffix(".live")?
                .to_string();
            let started = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            Some((id, started))
        })
        .collect()
}

/// Starts `claude` with `args` in `cwd`, `turn` on stdin, its output to
/// the run's files in `dir`, for the routine `r`: its process id, or why
/// it didn't start. Starts a process, so never on the server's task.
pub fn start(
    dir: &Path,
    id: &str,
    cwd: &str,
    args: &[String],
    turn: &str,
    name: &str,
) -> Result<u32, String> {
    use std::io::Write;
    let [out, err, _] = files(dir, id);
    let open = |p: &Path| {
        std::fs::File::create(p).map_err(|e| format!("couldn't write {}: {e}", p.display()))
    };
    let (out, err) = (open(&out)?, open(&err)?);
    let path = format!(
        "{}:{}",
        std::env::var("PATH").unwrap_or_default(),
        paths::home_join(".local/bin").display()
    );
    let mut cmd = std::process::Command::new("claude");
    cmd.args(args)
        .current_dir(cwd)
        .env("PATH", path)
        // Waits for claude.ai's connectors before the turn, which `-p`
        // otherwise starts while they are still connecting.
        .env("MCP_CONNECTION_NONBLOCKING", "false")
        .stdin(std::process::Stdio::piped())
        .stdout(out)
        .stderr(err);
    // A group of its own, so ending it ends whatever it started too (a
    // wrapper script's Claude, Claude's own tools).
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    crate::setup::plugin::headless(&mut cmd);
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err("Claude Code isn't installed".into());
        }
        Err(e) => return Err(format!("Claude didn't start ({e})")),
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(turn.as_bytes());
    }
    requests::count("Anthropic", "claude -p", &format!("routine {name}"));
    // Not waited on here: the tick watches it by its id, which a server
    // that replaces this one can too.
    Ok(child.id())
}

/// Whether the run's process has ended: reaped here when it is this
/// process's child (as it stays across an upgrade's exec), else gone.
pub fn ended(pid: u32, born: u64) -> bool {
    let Ok(pid) = libc::pid_t::try_from(pid) else {
        return true;
    };
    let mut status = 0;
    // SAFETY: waitpid with WNOHANG only reads the child's state.
    match unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) } {
        0 => false,
        n if n == pid => true,
        // Not this process's child (a server started again, or the
        // computer since): gone once no Claude has its id, a process of
        // another program having taken it up after a restart.
        _ => !is_run(pid as u32, born),
    }
}

/// When the process `pid` started, in milliseconds.
pub fn born_of(pid: u32) -> Option<u64> {
    let at = crate::mux::procinfo::started_at(pid)?;
    let ms = at.duration_since(std::time::UNIX_EPOCH).ok()?.as_millis();
    u64::try_from(ms).ok()
}

/// Whether the process `pid` is still the run's: a Claude, started when
/// the run's did (when that was read).
fn is_run(pid: u32, born: u64) -> bool {
    is_claude(pid) && (born == 0 || born_of(pid) == Some(born))
}

/// Whether the process `pid` is a Claude, as a pane's is recognized
/// (`agent::sessions::is_claude_name`: the binary by its name or version,
/// an npm install's script under node, or a shell running a script named
/// `claude`).
fn is_claude(pid: u32) -> bool {
    crate::mux::procinfo::command_of(pid).is_some_and(|command| {
        let name = Path::new(&command)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or(command);
        crate::agent::sessions::is_claude_name(&name, || crate::mux::procinfo::args_of(pid))
    })
}

/// Ends a run's process, gently or (`hard`) at once, only while it is
/// still the run's, so an id taken up by another process is never signaled.
pub fn stop(pid: u32, born: u64, hard: bool) {
    let Ok(id) = libc::pid_t::try_from(pid) else {
        return;
    };
    if !is_run(pid, born) {
        return;
    }
    let signal = if hard { libc::SIGKILL } else { libc::SIGTERM };
    // SAFETY: a signal to the run's own process group, which it leads.
    unsafe {
        libc::kill(-id, signal);
    }
}

/// A run's files taken away once its answer is recorded.
pub fn clear(dir: &Path, id: &str) {
    for f in files(dir, id) {
        let _ = std::fs::remove_file(f);
    }
}

/// Reads a file, at most `OUTPUT_MOST` bytes of it.
fn read_most(path: &Path) -> String {
    use std::io::Read;
    let mut bytes = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(OUTPUT_MOST).read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The most of a run's output read: a reply past it reads as none.
const OUTPUT_MOST: u64 = 4 * 1024 * 1024;

/// A run's answer from its files once it ended: its reply, or why there is
/// none. The files stay until the server has recorded it (`clear`), so a
/// restart meanwhile reads them again. Reads files of any size, so never
/// on the server's task.
pub fn answer(dir: &Path, id: &str) -> Result<Reply, String> {
    let [out, err, _] = files(dir, id);
    let stdout = read_most(&out);
    let stderr = read_most(&err);
    match read_reply(&stdout) {
        Some(reply) => Ok(reply),
        None => {
            let said = first_line(&stderr);
            let said = if said.is_empty() {
                first_line(&stdout)
            } else {
                said
            };
            Err(if said.is_empty() {
                "Claude exited without an answer".to_string()
            } else {
                said
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_names_its_prompt_model_tools_and_session() {
        let mut r = crate::routines::view::tests_routine();
        r.tools = vec!["Read".into(), "Bash(gh pr list:*)".into()];
        let a = args(&r);
        assert_eq!(
            a[..7],
            [
                "-p",
                "--output-format",
                "json",
                "--permission-mode",
                "dontAsk",
                "--setting-sources",
                ""
            ]
        );
        assert!(a[8].starts_with("Sort the new issues.\n\n"), "{}", a[8]);
        assert!(a[8].contains("only these tools: Read, Bash(gh pr list:*)."));
        assert!(!a.contains(&"--model".to_string()));
        assert!(!a.contains(&"--resume".to_string()));
        let at = a.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a[at + 1], "Read,Bash");
        let mut none = r.clone();
        none.tools = vec!["mcp__claude_ai_Slack__slack_read_channel".into()];
        let a2 = args(&none);
        let at = a2.iter().position(|x| x == "--tools").unwrap();
        assert_eq!(a2[at + 1], "", "no built-in tool at all");
        assert_eq!(
            a[a.len() - 3..],
            ["--allowedTools", "Read", "Bash(gh pr list:*)"]
        );

        r.model = "haiku".into();
        r.session = "s1".into();
        r.session_dir = r.dir.clone();
        let a = args(&r);
        let at = a.iter().position(|x| x == "--model").unwrap();
        assert_eq!(a[at + 1], "haiku");
        let at = a.iter().position(|x| x == "--resume").unwrap();
        assert_eq!(a[at + 1], "s1");
        // A session made in another directory isn't one `--resume` finds.
        r.session_dir = "/elsewhere".into();
        assert!(!args(&r).contains(&"--resume".to_string()));
    }

    #[test]
    fn a_rule_naming_the_notes_or_state_folder_follows_where_they_are() {
        let home = Path::new("/home/me");
        let usual = (home.join("Notes"), home.join(".local/state/switchboard"));
        let rule = "Write(~/Notes/Todos/**)";
        assert_eq!(placed(rule, home, &usual.0, &usual.1), rule);
        assert_eq!(
            placed(rule, home, Path::new("/data/notes/"), &usual.1),
            "Write(//data/notes/Todos/**)"
        );
        assert_eq!(
            placed(
                "Edit(~/.local/state/switchboard/meetings.json)",
                home,
                &usual.0,
                Path::new("/tmp/state")
            ),
            "Edit(//tmp/state/meetings.json)"
        );
        assert_eq!(
            placed(
                "Bash(gh search prs:*)",
                home,
                Path::new("/x"),
                Path::new("/y")
            ),
            "Bash(gh search prs:*)"
        );
    }

    #[test]
    fn its_reply_cost_session_and_refused_tools_are_read() {
        let out = r#"{"type":"result","subtype":"success","is_error":false,"result":"\nAll green.\nMore.","session_id":"abc","total_cost_usd":0.0412,"permission_denials":[{"tool_name":"Bash","tool_use_id":"t1","tool_input":{"command":"ls"}},{"tool_name":"Bash","tool_use_id":"t2"},{"tool_name":"Write"}]}"#;
        let r = read_reply(out).unwrap();
        assert_eq!(r.session, "abc");
        assert_eq!(first_line(&r.text), "All green.");
        assert_eq!(r.cost, 0.0412);
        assert!(!r.error);
        assert_eq!(r.denied, ["Bash", "Write"]);
        let failed = read_reply(r#"{"subtype":"error_max_turns","session_id":"x"}"#).unwrap();
        assert!(failed.error);
        assert!(read_reply("Not logged in").is_none());
    }
}
