//! The demo's Claude: a stand-in that looks enough like Claude Code in its
//! pane and tells the workspace what it is doing the way Claude Code does,
//! through `switchboard hook <event>` with the hook's JSON on stdin
//! (SessionStart, UserPromptSubmit, PreToolUse and PostToolUse with the
//! tools named, PermissionRequest, AskUserQuestion, Stop), so the sidebar,
//! mission control and the notifications show its tab working, waiting on
//! you and done. It reaches no model: it plays a script, one per tab
//! (`script`), then takes what you type and answers that it is a stand-in.
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
}

/// A turn: the prompt, its steps, and the reply it ends with.
struct Turn {
    prompt: Cow<'static, str>,
    steps: Vec<Step>,
    reply: &'static str,
    /// How long it rests after, before the next turn, in seconds; `None`
    /// waits for you to type the next prompt.
    rest: Option<u64>,
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
                rest: Some(25),
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
                rest: Some(25),
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
            rest: Some(40),
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
            rest: None,
        }],
        _ => Vec::new(),
    }
}

const DIM: &str = "\x1b[2m";
const BOLD: &str = "\x1b[1m";
const ORANGE: &str = "\x1b[38;2;215;119;87m";
const GREEN: &str = "\x1b[32m";
const RESET: &str = "\x1b[0m";

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

/// A choice, a key at a time as Claude Code takes one: a digit picks that
/// option at once (mission control answers a tab with the digit alone),
/// Enter the first; `None` once the terminal is gone.
fn read_choice() -> Option<usize> {
    use std::io::Read;
    let raw = crossterm::terminal::enable_raw_mode().is_ok();
    let mut byte = [0u8; 1];
    let chosen = loop {
        match std::io::stdin().read(&mut byte) {
            Ok(0) | Err(_) => break None,
            Ok(_) => match byte[0] {
                b'1'..=b'9' => break Some(usize::from(byte[0] - b'0')),
                b'\r' | b'\n' => break Some(1),
                // Ctrl-C and Ctrl-D end it, as they would Claude Code.
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

fn prompt_box() {
    out(&format!(
        "\n{DIM}╭──────────────────────────────────────────────────────────╮\n│{RESET} > {DIM}                                                       │\n╰──────────────────────────────────────────────────────────╯{RESET}\n  {DIM}? for shortcuts · the demo's Claude plays a script{RESET}\n> "
    ));
}

/// Plays one turn; `false` once the terminal is gone.
fn play(s: &mut Session, turn: &Turn) -> bool {
    out(&format!("\n{BOLD}> {}{RESET}\n\n", turn.prompt));
    s.user(&turn.prompt);
    s.hook("UserPromptSubmit", json!({ "prompt": turn.prompt }));
    // Once you say no to a tool, the turn ends saying so, not as scripted.
    let mut refused_any = false;
    for (n, step) in turn.steps.iter().enumerate() {
        if refused_any {
            break;
        }
        let call = format!("toolu_demo_{}_{n}", s.count);
        match step {
            Step::Say(text) => {
                out(&format!("{ORANGE}●{RESET} {text}\n\n"));
                s.assistant(json!({ "type": "text", "text": text }));
                sleep(2);
            }
            Step::Tool(name, input, output, secs) => {
                out(&format!(
                    "{GREEN}●{RESET} {BOLD}{name}{RESET}({})\n",
                    arg_of(input)
                ));
                s.assistant(
                    json!({ "type": "tool_use", "id": call, "name": name, "input": input }),
                );
                s.hook(
                    "PreToolUse",
                    json!({ "tool_name": name, "tool_input": input, "tool_use_id": call }),
                );
                sleep(*secs);
                out(&format!("  {DIM}⎿  {output}{RESET}\n\n"));
                s.tool_result(&call, output);
                s.hook(
                    "PostToolUse",
                    json!({ "tool_name": name, "tool_input": input, "tool_use_id": call, "tool_response": output }),
                );
            }
            Step::Permission(name, input, output) => {
                out(&format!(
                    "{GREEN}●{RESET} {BOLD}{name}{RESET}({})\n",
                    arg_of(input)
                ));
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
                out(&format!(
                    "\n{DIM}╭─{RESET} {BOLD}Bash command{RESET}\n{DIM}│{RESET}   {}\n{DIM}│{RESET}\n{DIM}│{RESET} Do you want to proceed?\n{DIM}│{RESET} {ORANGE}❯ 1. Yes{RESET}\n{DIM}│{RESET}   2. No, and tell Claude what to do differently\n{DIM}╰─ 1 or Enter yes, 2 no{RESET}\n",
                    arg_of(input)
                ));
                let Some(answer) = read_choice() else {
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
                out(&format!("  {DIM}⎿  {output}{RESET}\n\n"));
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
                out(&format!("\n{DIM}╭─{RESET} {BOLD}{question}{RESET}\n"));
                for (i, o) in options.iter().enumerate() {
                    let mark = if i == 0 {
                        format!("{ORANGE}❯")
                    } else {
                        " ".into()
                    };
                    out(&format!("{DIM}│{RESET} {mark} {}. {o}{RESET}\n", i + 1));
                }
                out(&format!("{DIM}╰─ a number picks, Enter the first{RESET}\n"));
                let Some(n) = read_choice() else {
                    return false;
                };
                let chosen = options.get(n - 1).unwrap_or(&options[0]);
                out(&format!("  {DIM}⎿  {chosen}{RESET}\n\n"));
                s.tool_result(&call, chosen);
                s.hook(
                    "PostToolUse",
                    json!({ "tool_name": "AskUserQuestion", "tool_input": input, "tool_use_id": call }),
                );
            }
        }
    }
    let reply = if refused_any {
        "Understood, I left it as it is. What should I do differently?"
    } else {
        turn.reply
    };
    out(&format!("{ORANGE}●{RESET} {reply}\n"));
    s.assistant(json!({ "type": "text", "text": reply }));
    s.hook(
        "Stop",
        json!({ "stop_hook_active": false, "last_assistant_message": reply }),
    );
    true
}

/// Runs the stand-in Claude on `name`'s script.
pub fn run(name: &str) -> ExitCode {
    let mut s = Session::start();
    out(&format!(
        "{ORANGE}✻{RESET} {BOLD}Welcome to Claude Code{RESET} {DIM}(the demo's stand-in){RESET}\n\n  {DIM}cwd: {}{RESET}\n",
        s.cwd
    ));
    s.hook("SessionStart", json!({ "source": "startup" }));
    let turns = script(name);
    // A pause first, so a demo just opened shows its tabs start.
    sleep(2);
    // A script whose every turn rests before the next plays again and
    // again; one that ends waiting on you takes what you type after it.
    let repeats = !turns.is_empty() && turns.iter().all(|t| t.rest.is_some());
    loop {
        for turn in &turns {
            if !play(&mut s, turn) {
                return ExitCode::SUCCESS;
            }
            if let Some(secs) = turn.rest {
                sleep(secs);
            }
        }
        if !repeats {
            break;
        }
    }
    loop {
        // Past its script, it takes what you type.
        prompt_box();
        let Some(prompt) = read_line() else {
            return ExitCode::SUCCESS;
        };
        if prompt.is_empty() {
            continue;
        }
        let typed = Turn {
            prompt: prompt.into(),
            steps: Vec::new(),
            reply: "I'm the demo's stand-in for Claude: I play a script and reach no model, so I can't do that here. Install Switchboard and run it with Claude Code to try it for real.",
            rest: None,
        };
        sleep(1);
        if !play(&mut s, &typed) {
            return ExitCode::SUCCESS;
        }
    }
}
