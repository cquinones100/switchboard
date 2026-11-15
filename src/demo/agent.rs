//! The demo's coding agent: a stand-in drawn as a generic terminal coding
//! agent (a welcome box, `●` before each tool call and reply with the
//! result under `└`, a rounded input box pinned to the pane's bottom rows
//! with `? for shortcuts` and the statusline under it, the permission and
//! question dialogs in its place), which tells the workspace what it is
//! doing the way Claude Code does, through `switchboard hook <event>` with
//! the hook's JSON on stdin (SessionStart, UserPromptSubmit, PreToolUse and
//! PostToolUse with the tools named, PermissionRequest, AskUserQuestion,
//! Stop), so the sidebar, mission control and the notifications show its
//! tab working, waiting on you and done. It reaches no model: it plays a
//! script, one per tab (`script`), then takes what you type and answers
//! that it is a stand-in.
//!
//! The scripts put a tab in each of the sidebar's groups: three that settle
//! in Needs you, one working on and on, one in a review loop, one in a CI
//! loop, one holding its repository's landing lock (`land-lock take`, run
//! for real in the demo's repository) and one resumed on a merged PR's
//! branch, its turn shown as history with no hooks, so it is idle rather
//! than done.
//!
//! The input box stays in the pane's bottom rows (`Screen`): a scroll
//! region ends above it, so the conversation scrolls in the rows above
//! without reaching it, with a spinner line over it while a turn runs. The
//! statusline in it is `switchboard statusline`, run as Claude Code runs it.
//!
//! It writes a session file for its process (`~/.claude/sessions/<pid>.json`),
//! which is how the workspace knows a pane runs Claude, and a transcript
//! where Claude Code keeps one, which the sidebar reads the last turn and
//! reply from (src/sidebar/claude.rs).

use std::borrow::Cow;
use std::fs;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use crate::ui::draw;
use crate::util;

/// One step of a turn.
enum Step {
    /// A tool run: its name, what it was given, what it printed, and how
    /// long it takes.
    Tool(&'static str, Value, &'static str, u64),
    /// A tool that waits for your yes first.
    Permission(&'static str, Value, &'static str),
    /// A question, with its options.
    Ask(&'static str, &'static [&'static str]),
    /// Text written as it works.
    Say(&'static str),
    /// A `switchboard` command run for real, saying nothing, as a skill's
    /// script runs one (`land-lock take`).
    Run(&'static [&'static str]),
}

/// What a turn does once its steps are played.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pace {
    /// Rests this many seconds before the next turn.
    Rest(u64),
    /// Waits for you to type the next prompt.
    Wait,
    /// Plays its steps again and again, never finishing.
    Endless,
    /// Already played before the session was resumed: shown as history,
    /// with no hooks and no pauses.
    Past,
}

/// A turn: the prompt, its steps, and the reply it ends with.
struct Turn {
    prompt: Cow<'static, str>,
    steps: Vec<Step>,
    reply: &'static str,
    pace: Pace,
}

fn script(name: &str) -> Vec<Turn> {
    use Step::*;
    match name {
        "rate-limit" => vec![
            Turn {
                prompt: "Rate-limit the login endpoint (ABC-12): ten attempts a minute per address"
                    .into(),
                steps: vec![
                    Tool(
                        "Read",
                        json!({ "file_path": "src/auth/login.ts" }),
                        "Read 84 lines",
                        3,
                    ),
                    Say(
                        "The login handler has no limiter yet. I'll add one in src/auth/limit.ts and use it there.",
                    ),
                    Tool(
                        "Write",
                        json!({ "file_path": "src/auth/limit.ts" }),
                        "Wrote 41 lines to src/auth/limit.ts",
                        4,
                    ),
                    Tool(
                        "Edit",
                        json!({ "file_path": "src/auth/login.ts" }),
                        "Updated src/auth/login.ts with 6 additions",
                        3,
                    ),
                    Tool(
                        "Bash",
                        json!({ "command": "npm test -- auth", "description": "Run the auth tests" }),
                        "Tests: 18 passed, 18 total",
                        12,
                    ),
                ],
                reply: "Login attempts are now limited to ten a minute for each address, answering 429 with a Retry-After header. The auth tests pass (18).",
                pace: Pace::Rest(25),
            },
            Turn {
                prompt: "Add a test for the Retry-After header".into(),
                steps: vec![
                    Tool(
                        "Edit",
                        json!({ "file_path": "test/auth/limit.test.ts" }),
                        "Updated test/auth/limit.test.ts with 22 additions",
                        4,
                    ),
                    Tool(
                        "Bash",
                        json!({ "command": "npm test -- limit", "description": "Run the limiter's tests" }),
                        "Tests: 4 passed, 4 total",
                        10,
                    ),
                ],
                reply: "Added a test that the eleventh attempt gets a 429 whose Retry-After counts down to the next minute.",
                pace: Pace::Rest(25),
            },
        ],
        "dark-mode" => vec![Turn {
            prompt: "The snapshot test fails on the dark mode PR; fix it and push".into(),
            steps: vec![
                Tool(
                    "Bash",
                    json!({ "command": "npm test -- snapshot", "description": "Run the snapshot tests" }),
                    "1 failed: Settings › Appearance matches its snapshot",
                    8,
                ),
                Say(
                    "The snapshot still has the old colors. Updating it, since the new ones are intended.",
                ),
                Tool(
                    "Bash",
                    json!({ "command": "npm test -- snapshot -u", "description": "Update the snapshot" }),
                    "1 snapshot updated",
                    6,
                ),
                Permission(
                    "Bash",
                    json!({ "command": "git push", "description": "Push the fix to the PR" }),
                    "Pushed to abc-15-dark-mode-toggle",
                ),
            ],
            reply: "The snapshot is updated for the new colors and pushed; CI is running again.",
            pace: Pace::Rest(40),
        }],
        "api-guide" => vec![Turn {
            prompt: "Address Sam's review on the quickstart guide".into(),
            steps: vec![
                Tool(
                    "Read",
                    json!({ "file_path": "guides/quickstart.md" }),
                    "Read 120 lines",
                    2,
                ),
                Tool(
                    "Edit",
                    json!({ "file_path": "guides/quickstart.md" }),
                    "Updated guides/quickstart.md with 14 additions",
                    3,
                ),
                Ask(
                    "The guide shows curl and JavaScript. Add a Python example too?",
                    &["Yes, add Python", "No, keep the two"],
                ),
            ],
            reply: "Sam's points are addressed, and the guide is approved. It's ready to merge when you are.",
            pace: Pace::Wait,
        }],
        "redis" => vec![Turn {
            prompt: "Move the sessions to Redis (ABC-9), keeping the database's as a fallback"
                .into(),
            steps: vec![
                Tool(
                    "Read",
                    json!({ "file_path": "src/session/store.ts" }),
                    "Read 132 lines",
                    4,
                ),
                Say(
                    "Sessions are read and written in one place, so a Redis store can stand behind the same interface.",
                ),
                Tool(
                    "Write",
                    json!({ "file_path": "src/session/redis.ts" }),
                    "Wrote 58 lines to src/session/redis.ts",
                    6,
                ),
                Tool(
                    "Edit",
                    json!({ "file_path": "src/session/store.ts" }),
                    "Updated src/session/store.ts with 12 additions and 4 removals",
                    5,
                ),
                Tool(
                    "Bash",
                    json!({ "command": "npm test -- session", "description": "Run the session tests" }),
                    "Tests: 2 failed, 24 passed, 26 total",
                    14,
                ),
                Say(
                    "Two tests expect a session to outlive a restart of the store; giving Redis's a TTL that matches.",
                ),
                Tool(
                    "Edit",
                    json!({ "file_path": "src/session/redis.ts" }),
                    "Updated src/session/redis.ts with 3 additions",
                    5,
                ),
            ],
            reply: "",
            pace: Pace::Endless,
        }],
        "review" => vec![Turn {
            prompt: "/review-loop".into(),
            steps: vec![
                Say("Round 1: reviewing the branch against main."),
                Tool(
                    "Bash",
                    json!({ "command": "switchboard codex-review --base main", "description": "Review the branch" }),
                    "1 finding: medium, src/email/worker.ts:31 retries without a limit",
                    18,
                ),
                Tool(
                    "Read",
                    json!({ "file_path": "src/email/worker.ts" }),
                    "Read 76 lines",
                    3,
                ),
                Tool(
                    "Edit",
                    json!({ "file_path": "src/email/worker.ts" }),
                    "Updated src/email/worker.ts with 4 additions and 1 removal",
                    4,
                ),
                Tool(
                    "Bash",
                    json!({ "command": "npm test -- email", "description": "Run the email tests" }),
                    "Tests: 11 passed, 11 total",
                    10,
                ),
                Say("Round 2: reviewing the fix."),
                Tool(
                    "Bash",
                    json!({ "command": "switchboard codex-review --base main", "description": "Review the branch" }),
                    "Clean: no findings",
                    18,
                ),
            ],
            reply: "",
            pace: Pace::Endless,
        }],
        "ci" => vec![Turn {
            prompt: "/ci-loop".into(),
            steps: vec![
                Tool(
                    "Bash",
                    json!({ "command": "switchboard wait-ci", "description": "Wait for the PR's checks" }),
                    "lint passed; test still running (3 of 5 checks done)",
                    20,
                ),
                Say("The tests are still running; waiting for them before anything else."),
                Tool(
                    "Bash",
                    json!({ "command": "gh pr checks 345", "description": "Read the PR's checks" }),
                    "test  pending  4m\nlint  pass     52s",
                    6,
                ),
            ],
            reply: "",
            pace: Pace::Endless,
        }],
        "landing" => vec![Turn {
            prompt: "/queue-and-clean".into(),
            steps: vec![
                Run(&["land-lock", "take"]),
                Run(&["land-lock", "phase", "rebasing"]),
                Tool(
                    "Bash",
                    json!({ "command": "git rebase origin/main", "description": "Rebase onto main" }),
                    "Successfully rebased and updated refs/heads/abc-19-changelog-page.",
                    5,
                ),
                Run(&["land-lock", "phase", "testing"]),
                Tool(
                    "Bash",
                    json!({ "command": "npm test", "description": "Run the tests" }),
                    "Tests: 42 passed, 42 total",
                    16,
                ),
                Run(&["land-lock", "phase", "reviewing"]),
                Tool(
                    "Bash",
                    json!({ "command": "switchboard codex-review --recorded", "description": "Check the review" }),
                    "Reviewed clean at this commit",
                    8,
                ),
            ],
            reply: "",
            pace: Pace::Endless,
        }],
        "merged" => vec![Turn {
            prompt: "Fix the avatar upload on Safari (ABC-11) and land it".into(),
            steps: vec![
                Tool(
                    "Read",
                    json!({ "file_path": "src/settings/avatar.ts" }),
                    "Read 64 lines",
                    0,
                ),
                Tool(
                    "Edit",
                    json!({ "file_path": "src/settings/avatar.ts" }),
                    "Updated src/settings/avatar.ts with 9 additions and 2 removals",
                    0,
                ),
                Tool(
                    "Bash",
                    json!({ "command": "npm test -- avatar", "description": "Run the avatar tests" }),
                    "Tests: 7 passed, 7 total",
                    0,
                ),
            ],
            reply: "Safari sends the file without a type, so it is now read from the file's first bytes. The PR is merged.",
            pace: Pace::Past,
        }],
        _ => Vec::new(),
    }
}

const BOLD: &str = "\x1b[1m";
const GRAY: &str = "\x1b[38;5;244m";
const ACCENT: &str = "\x1b[36m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

/// The words the spinner cycles through while a turn runs.
const SPINNER: &[&str] = &["Thinking", "Working", "Reading", "Writing"];

struct Session {
    id: String,
    transcript: PathBuf,
    cwd: String,
    file: PathBuf,
    bin: PathBuf,
    count: u64,
}

impl Session {
    fn start() -> Session {
        let cwd = std::env::current_dir()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let pid = std::process::id();
        let id = format!("00000000-abc-4000-8000-{pid:012}");
        let claude = crate::paths::claude_dir();
        let transcript = claude
            .join("projects")
            .join(crate::sidebar::claude::project_dir_name(&cwd))
            .join(format!("{id}.jsonl"));
        if let Some(dir) = transcript.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let file = claude.join("sessions").join(format!("{pid}.json"));
        let _ = fs::create_dir_all(claude.join("sessions"));
        let _ = util::write_atomic(
            &file,
            util::json_compact(&json!({
                "pid": pid, "sessionId": id, "cwd": cwd, "startedAt": util::now_ms(),
                "kind": "interactive", "entrypoint": "cli",
            })),
        );
        Session {
            id,
            transcript,
            cwd,
            file,
            bin: std::env::current_exe().unwrap_or_else(|_| "switchboard".into()),
            count: 0,
        }
    }

    /// Sends a hook as Claude Code does, and waits for it.
    fn hook(&self, event: &str, mut payload: Value) {
        payload["session_id"] = json!(self.id);
        payload["transcript_path"] = json!(self.transcript.to_string_lossy());
        payload["cwd"] = json!(self.cwd);
        payload["hook_event_name"] = json!(event);
        payload["permission_mode"] = json!("default");
        let child = Command::new(&self.bin)
            .args(["hook", event])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        if let Ok(mut child) = child {
            if let Some(mut stdin) = child.stdin.take() {
                let _ = stdin.write_all(payload.to_string().as_bytes());
            }
            let _ = child.wait();
        }
    }

    /// Runs a `switchboard` command as this session's, as a skill's
    /// script does in Claude Code's Bash tool.
    fn run(&self, args: &[&str]) {
        let _ = Command::new(&self.bin)
            .args(args)
            .env("CLAUDE_CODE_SESSION_ID", &self.id)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// The statusline, as Claude Code gets it: `switchboard statusline`
    /// given the session's JSON, its context filling as the session goes.
    fn statusline(&self) -> String {
        let used = (8.0 + self.count as f64 * 0.6).min(85.0);
        let payload = json!({
            "session_id": self.id,
            "transcript_path": self.transcript.to_string_lossy(),
            "cwd": self.cwd,
            "model": { "id": "claude-demo", "display_name": "Demo model" },
            "workspace": { "current_dir": self.cwd, "project_dir": self.cwd },
            "context_window": { "used_percentage": used },
        });
        let child = Command::new(&self.bin)
            .arg("statusline")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else {
            return String::new();
        };
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.to_string().as_bytes());
        }
        child
            .wait_with_output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .unwrap_or_default()
    }

    fn write(&mut self, mut entry: Value) {
        self.count += 1;
        entry["timestamp"] = json!(
            jiff::Timestamp::now()
                .strftime("%Y-%m-%dT%H:%M:%S%.3fZ")
                .to_string()
        );
        entry["sessionId"] = json!(self.id);
        entry["uuid"] = json!(format!("demo-{}-{}", self.id, self.count));
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.transcript)
        {
            let _ = writeln!(f, "{entry}");
        }
    }

    fn user(&mut self, prompt: &str) {
        self.write(json!({ "type": "user", "message": { "role": "user", "content": prompt } }));
    }

    fn assistant(&mut self, content: Value) {
        let id = format!("msg_demo_{}", self.count);
        self.write(json!({ "type": "assistant", "message": {
            "id": id, "role": "assistant", "model": "claude-demo", "content": [content] } }));
    }

    fn tool_result(&mut self, call: &str, output: &str) {
        self.write(
            json!({ "type": "user", "message": { "role": "user", "content": [
            { "type": "tool_result", "tool_use_id": call, "content": output } ] } }),
        );
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.file);
    }
}

fn sleep(secs: u64) {
    std::thread::sleep(Duration::from_secs(secs));
}

fn out(text: &str) {
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "{text}");
    let _ = stdout.flush();
}

/// The pane's size, columns and rows.
fn size() -> (usize, usize) {
    crossterm::terminal::size()
        .map(|(c, r)| (usize::from(c).clamp(24, 400), usize::from(r).clamp(12, 200)))
        .unwrap_or((80, 24))
}

/// Lines in a rounded box `width` columns wide, its edge in `edge`: each
/// line a style and its text, cut to fit (its own colors dropped when it
/// is cut), so the box's height is its line count.
fn boxed(lines: &[(&str, String)], width: usize, edge: &str) -> Vec<String> {
    let inner = width.saturating_sub(4).max(1);
    let mut rows = vec![format!("{edge}╭{}╮{RESET}", "─".repeat(inner + 2))];
    for (style, text) in lines {
        let plain = strip(text);
        let fitted = match inner.checked_sub(draw::width(&plain)) {
            Some(pad) => format!("{text}{RESET}{style}{}", " ".repeat(pad)),
            None => draw::fit(&plain, inner),
        };
        rows.push(format!(
            "{edge}│{RESET} {style}{fitted}{RESET} {edge}│{RESET}"
        ));
    }
    rows.push(format!("{edge}╰{}╯{RESET}", "─".repeat(inner + 2)));
    rows
}

/// The pane as a coding agent keeps it: the conversation scrolling in the
/// rows above, and a block pinned to the pane's bottom rows (the input
/// box, or a dialog in its place), held there by a scroll region that
/// ends above it, so what is written scrolls without reaching it.
///
/// Showing a block paints the whole pane again from what was written (its
/// last `KEPT` pieces): the workspace lays a tab opened in the background
/// out a moment after its program starts, and a terminal resized moves its
/// rows, so a screen drawn by counting rows alone would be left torn; the
/// next block mends it.
struct Screen {
    /// The row the conversation's cursor is on, from 1; it is always at a
    /// line's start, since everything written ends a line.
    row: usize,
    /// The last row of the conversation's rows, above the block.
    bottom: usize,
    /// The statusline, read again each turn.
    status: String,
    /// The pane's size when last painted, columns and rows.
    size: (usize, usize),
    written: std::collections::VecDeque<String>,
    block: Vec<String>,
}

/// How many pieces of the conversation are kept to paint again.
const KEPT: usize = 200;

/// The rows `text` takes in `cols` columns, wrapped lines included.
fn rows_of(text: &str, cols: usize) -> usize {
    text.strip_suffix('\n')
        .unwrap_or(text)
        .split('\n')
        .map(|line| draw::width(&strip(line)).max(1).div_ceil(cols))
        .sum()
}

impl Screen {
    fn new() -> Screen {
        let size = size();
        Screen {
            row: 1,
            bottom: size.1,
            status: String::new(),
            size,
            written: std::collections::VecDeque::new(),
            block: Vec::new(),
        }
    }

    /// Waits for something typed, painting again whenever the pane is
    /// resized meanwhile; `at` puts the cursor back where typing goes.
    fn wait_for_input(&mut self, at: impl Fn(&Screen)) {
        loop {
            let mut fd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: poll reads one pollfd that lives through the call.
            let ready = unsafe { libc::poll(&mut fd, 1, 300) };
            if ready != 0 {
                return;
            }
            if size() != self.size {
                // A resize comes in steps; paint once it has settled.
                std::thread::sleep(Duration::from_millis(200));
                let block = self.block.clone();
                self.show(&block);
                at(self);
            }
        }
    }

    /// Writes `text`, whole lines, in the conversation's rows.
    fn print(&mut self, text: &str) {
        if self.written.len() == KEPT {
            self.written.pop_front();
        }
        self.written.push_back(text.to_string());
        self.write(text);
    }

    fn write(&mut self, text: &str) {
        out(&format!("\x1b[{};1H{text}", self.row));
        // Past the region's bottom, printing scrolls the rows above.
        self.row = (self.row + rows_of(text, self.size.0)).min(self.bottom);
    }

    /// Paints the pane again: the conversation's last pieces that fit in
    /// the rows above `block`, then `block` in the bottom rows.
    fn show(&mut self, block: &[String]) {
        self.size = size();
        let (cols, rows) = self.size;
        let height = block.len().min(rows.saturating_sub(2));
        self.bottom = rows - height;
        self.block = block.to_vec();
        // The pieces from the last back, as many as fill the rows above.
        let mut used = 0;
        let mut first = self.written.len();
        while first > 0 && used < self.bottom {
            first -= 1;
            used += rows_of(&self.written[first], cols);
        }
        out(&format!("\x1b[r\x1b[2J\x1b[1;{}r", self.bottom));
        self.row = 1;
        let tail: Vec<String> = self.written.iter().skip(first).cloned().collect();
        for text in &tail {
            self.write(text);
        }
        out(&format!("\x1b[{};1H", self.bottom + 1));
        out(&block[block.len() - height..].join("\r\n"));
        out(&format!("\x1b[{};1H", self.row));
    }

    /// The input box, with the spinner over it while a turn runs (`busy`
    /// its word), `? for shortcuts` and the statusline under it.
    fn input(&mut self, busy: Option<&str>) {
        let (cols, _) = size();
        let mut rows = Vec::new();
        if let Some(word) = busy {
            rows.push(format!(
                "{ACCENT}◆ {word}…{RESET} {GRAY}(esc to interrupt){RESET}"
            ));
        }
        rows.push(String::new());
        rows.extend(boxed(&[("", "> ".into())], cols, GRAY));
        rows.push(format!("  {GRAY}? for shortcuts{RESET}"));
        rows.push(self.status.clone());
        self.show(&rows);
    }

    /// Takes a prompt typed into the input box, `None` once the terminal
    /// is gone.
    fn read_prompt(&mut self) -> Option<String> {
        self.input(None);
        // Into the box, after its `> `: the box's middle row is the
        // fourth from the pane's bottom (the box's last row, the hint and
        // the statusline under it).
        let into_box = |s: &Screen| out(&format!("\x1b[{};5H", s.size.1 - 3));
        into_box(self);
        self.wait_for_input(into_box);
        let line = read_line();
        out(&format!("\x1b[{};1H", self.row));
        line
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        out("\x1b[r");
    }
}

/// A choice, a key at a time as a coding agent takes one: a digit picks
/// that option at once (mission control answers a tab with the digit
/// alone), Enter the first; `None` once the terminal is gone.
fn read_choice(screen: &mut Screen) -> Option<usize> {
    use std::io::Read;
    // Raw before waiting, so a key alone wakes the wait.
    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    screen.wait_for_input(|_| {});
    let mut byte = [0u8; 1];
    let chosen = loop {
        match std::io::stdin().read(&mut byte) {
            Ok(0) | Err(_) => break None,
            Ok(_) => match byte[0] {
                b'1'..=b'9' => break Some(usize::from(byte[0] - b'0')),
                b'\r' | b'\n' => break Some(1),
                // Ctrl-C and Ctrl-D end it, as they would a coding agent.
                3 | 4 => break None,
                _ => {}
            },
        }
    };
    if raw {
        let _ = crossterm::terminal::disable_raw_mode();
    }
    chosen
}

/// A line typed, `None` once the terminal is gone.
fn read_line() -> Option<String> {
    let mut line = String::new();
    match std::io::stdin().lock().read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

fn arg_of(input: &Value) -> String {
    ["command", "file_path", "pattern"]
        .iter()
        .find_map(|k| input.get(*k).and_then(Value::as_str))
        .unwrap_or("")
        .to_string()
}

/// A tool call's line, `●` in `mark`'s color.
fn call_line(mark: &str, name: &str, input: &Value) -> String {
    format!("{mark}●{RESET} {BOLD}{name}{RESET}({})", arg_of(input))
}

/// A tool's result under its call, its first line after `└`.
fn result_lines(output: &str) -> String {
    let mut text = String::new();
    for (i, line) in output.lines().enumerate() {
        let lead = if i == 0 { "  └ " } else { "    " };
        text.push_str(&format!("{GRAY}{lead}{line}{RESET}\n"));
    }
    text.push('\n');
    text
}

/// The permission dialog for a Bash command.
fn permission_rows(input: &Value) -> Vec<String> {
    let (cols, _) = size();
    let mut lines: Vec<(&str, String)> = vec![
        (BOLD, "Bash command".into()),
        ("", String::new()),
        ("", format!("  {}", arg_of(input))),
    ];
    if let Some(about) = input.get("description").and_then(Value::as_str) {
        lines.push((GRAY, format!("  {about}")));
    }
    lines.extend([
        ("", String::new()),
        ("", "Do you want to proceed?".into()),
        (ACCENT, "❯ 1. Yes".into()),
        (
            "",
            "  2. No, and tell the agent what to do differently (esc)".into(),
        ),
    ]);
    boxed(&lines, cols, ACCENT)
}

/// The question dialog: a rule, the header, the question and the numbered
/// options.
fn question_rows(header: &str, question: &str, options: &[&str]) -> Vec<String> {
    let (cols, _) = size();
    let mut rows = vec![
        format!("{GRAY}{}{RESET}", "─".repeat(cols.saturating_sub(1))),
        format!(" {BOLD}☐ {header}{RESET}"),
        String::new(),
    ];
    for line in draw::wrap(question, cols.saturating_sub(2)) {
        rows.push(format!("{BOLD}{line}{RESET}"));
    }
    rows.push(String::new());
    for (i, o) in options.iter().enumerate() {
        let text = draw::cut(&format!("{}. {o}", i + 1), cols.saturating_sub(3));
        rows.push(if i == 0 {
            format!("{ACCENT}❯ {text}{RESET}")
        } else {
            format!("  {text}")
        });
    }
    rows.push(String::new());
    rows.push(format!(
        "{GRAY}Enter to select · 1-{} to choose{RESET}",
        options.len()
    ));
    rows
}

/// Plays one turn; `false` once the terminal is gone. An endless turn
/// plays until then.
fn play(s: &mut Session, screen: &mut Screen, turn: &Turn) -> bool {
    let live = turn.pace != Pace::Past;
    screen.print(&format!("{GRAY}>{RESET} {}\n\n", turn.prompt));
    s.user(&turn.prompt);
    if live {
        s.hook("UserPromptSubmit", json!({ "prompt": turn.prompt }));
        screen.status = s.statusline();
    }
    let busy = |n: usize| SPINNER[n % SPINNER.len()];
    // Once you say no to a tool, the turn ends saying so, not as scripted.
    let mut refused_any = false;
    let mut played = 0;
    loop {
        for step in &turn.steps {
            if refused_any {
                break;
            }
            let call = format!("toolu_demo_{}_{played}", s.count);
            played += 1;
            match step {
                Step::Run(args) => {
                    if live {
                        s.run(args);
                    }
                }
                Step::Say(text) => {
                    screen.print(&format!("● {text}\n\n"));
                    s.assistant(json!({ "type": "text", "text": text }));
                    if live {
                        screen.input(Some(busy(played)));
                        sleep(2);
                    }
                }
                Step::Tool(name, input, output, secs) => {
                    s.assistant(
                        json!({ "type": "tool_use", "id": call, "name": name, "input": input }),
                    );
                    screen.print(&format!("{}\n", call_line(GREEN, name, input)));
                    if live {
                        s.hook(
                            "PreToolUse",
                            json!({ "tool_name": name, "tool_input": input, "tool_use_id": call }),
                        );
                        screen.input(Some(busy(played)));
                        sleep(*secs);
                    }
                    screen.print(&result_lines(output));
                    s.tool_result(&call, output);
                    if live {
                        s.hook(
                            "PostToolUse",
                            json!({ "tool_name": name, "tool_input": input, "tool_use_id": call, "tool_response": output }),
                        );
                    }
                }
                Step::Permission(name, input, output) => {
                    screen.print(&format!("{}\n", call_line(GRAY, name, input)));
                    s.assistant(
                        json!({ "type": "tool_use", "id": call, "name": name, "input": input }),
                    );
                    s.hook(
                        "PreToolUse",
                        json!({ "tool_name": name, "tool_input": input, "tool_use_id": call }),
                    );
                    s.hook(
                        "PermissionRequest",
                        json!({ "tool_name": name, "tool_input": input }),
                    );
                    s.hook(
                        "Notification",
                        json!({ "message": format!("Claude needs your permission to use {name}"), "notification_type": "permission_prompt" }),
                    );
                    screen.show(&permission_rows(input));
                    let Some(answer) = read_choice(screen) else {
                        return false;
                    };
                    // The second option is a refusal: the tool doesn't run.
                    let refused = answer == 2;
                    refused_any |= refused;
                    let (output, event) = if refused {
                        ("Not run: you said no", "PostToolUseFailure")
                    } else {
                        (*output, "PostToolUse")
                    };
                    screen.print(&result_lines(output));
                    screen.input(None);
                    s.tool_result(&call, output);
                    s.hook(
                        event,
                        json!({ "tool_name": name, "tool_input": input, "tool_use_id": call, "tool_response": output }),
                    );
                }
                Step::Ask(question, options) => {
                    let input = json!({ "questions": [{ "question": question, "header": "Choice",
                        "multiSelect": false,
                        "options": options.iter().map(|o| json!({ "label": o, "description": "" })).collect::<Vec<_>>() }] });
                    s.assistant(json!({ "type": "tool_use", "id": call, "name": "AskUserQuestion", "input": input }));
                    s.hook(
                        "PreToolUse",
                        json!({ "tool_name": "AskUserQuestion", "tool_input": input, "tool_use_id": call }),
                    );
                    screen.show(&question_rows("Choice", question, options));
                    let Some(n) = read_choice(screen) else {
                        return false;
                    };
                    let chosen = options.get(n - 1).unwrap_or(&options[0]);
                    screen.print(&format!(
                        "{GREEN}●{RESET} {BOLD}You answered:{RESET}\n{}",
                        result_lines(&format!("{question} → {chosen}"))
                    ));
                    screen.input(None);
                    s.tool_result(&call, chosen);
                    s.hook(
                        "PostToolUse",
                        json!({ "tool_name": "AskUserQuestion", "tool_input": input, "tool_use_id": call }),
                    );
                }
            }
        }
        if turn.pace != Pace::Endless {
            break;
        }
    }
    let reply = if refused_any {
        "Understood, I left it as it is. What should I do differently?"
    } else {
        turn.reply
    };
    screen.print(&format!("● {reply}\n\n"));
    s.assistant(json!({ "type": "text", "text": reply }));
    if live {
        s.hook(
            "Stop",
            json!({ "stop_hook_active": false, "last_assistant_message": reply }),
        );
        screen.status = s.statusline();
    }
    screen.input(None);
    true
}

/// The welcome box.
fn welcome(cwd: &str) -> String {
    let lines: Vec<(&str, String)> = vec![
        ("", format!("{ACCENT}◆{RESET} {BOLD}Coding agent")),
        ("", String::new()),
        (GRAY, format!("  cwd: {cwd}")),
    ];
    let widest = lines
        .iter()
        .map(|(_, t)| draw::width(&strip(t)))
        .max()
        .unwrap_or(0);
    let rows = boxed(&lines, (widest + 4).min(size().0), ACCENT);
    format!("{}\n\n", rows.join("\n"))
}

/// `text` without its color codes, for measuring it.
fn strip(text: &str) -> String {
    let mut plain = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            for d in chars.by_ref() {
                if d.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

/// Runs the stand-in agent on `name`'s script.
pub fn run(name: &str) -> ExitCode {
    let mut s = Session::start();
    let turns = script(name);
    let resumed = !turns.is_empty() && turns.iter().all(|t| t.pace == Pace::Past);
    let mut screen = Screen::new();
    screen.print(&welcome(&crate::paths::tilde(&s.cwd)));
    s.hook(
        "SessionStart",
        json!({ "source": if resumed { "resume" } else { "startup" } }),
    );
    screen.status = s.statusline();
    screen.input(None);
    // A pause first, so a demo just opened shows its tabs start.
    if !resumed {
        sleep(2);
    }
    // A script whose every turn rests before the next plays again and
    // again; one that ends waiting on you takes what you type after it.
    let repeats = !turns.is_empty() && turns.iter().all(|t| matches!(t.pace, Pace::Rest(_)));
    loop {
        for turn in &turns {
            if !play(&mut s, &mut screen, turn) {
                return ExitCode::SUCCESS;
            }
            if let Pace::Rest(secs) = turn.pace {
                sleep(secs);
            }
        }
        if !repeats {
            break;
        }
    }
    loop {
        // Past its script, it takes what you type.
        let Some(prompt) = screen.read_prompt() else {
            return ExitCode::SUCCESS;
        };
        if prompt.is_empty() {
            screen.input(None);
            continue;
        }
        let typed = Turn {
            prompt: prompt.into(),
            steps: Vec::new(),
            reply: "I'm the demo's stand-in coding agent: I play a script and reach no model, so I can't do that here. Install Switchboard and run it with Claude Code to try it for real.",
            pace: Pace::Wait,
        };
        sleep(1);
        if !play(&mut s, &mut screen, &typed) {
            return ExitCode::SUCCESS;
        }
    }
}
