//! One line in a tab's thread when one of its sessions gets somewhere that
//! matters (lib/slack-relay/milestones.js): a commit, a push, or a pull
//! request opened. Every Claude tab's transcript is followed from where it
//! was when the relay first saw it, and a Bash call that looks like one of
//! those is reported when its result comes back. Test runs are not: one
//! failing is routine while Claude works, and they were most of what the
//! relay sent.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value;

use super::Cx;
use super::alerts::notify;
use super::homes::home_of;
use super::post::{PostOpts, post};
use super::sending::{self, Kind, Send, Sent};
use super::state::Target;
use super::tabs::{Tab, label};

/// gh and git count only where a command starts (a line's start, or after
/// ; && || | ( or $( ), so text that only mentions them isn't taken for a
/// push. git's own push and commit, after any of git's options (-C dir,
/// -c k=v, --no-pager…), not a subcommand's, such as `git stash push`.
static KINDS: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    const AT: &str = r"(?:^|[;&|(]|\$\()\s*";
    const GIT: &str = r"git(?:\s+-[Cc]\s+\S+|\s+--?[\w-]+(?:=\S+)?)*\s+";
    vec![
        (
            "pr",
            Regex::new(&format!(r"(?m){AT}gh\s+pr\s+create\b")).expect("a regex"),
        ),
        (
            "push",
            Regex::new(&format!(r"(?m){AT}{GIT}push\b")).expect("a regex"),
        ),
        (
            "commit",
            Regex::new(&format!(r"(?m){AT}{GIT}commit\b")).expect("a regex"),
        ),
    ]
});

/// A command with its heredocs' bodies left out: a heredoc is text handed
/// to a command, not commands, so writing a file that mentions `git push`
/// pushes nothing.
pub fn without_heredocs(command: &str) -> String {
    static START: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<<-?\s*(['"]?)(\w+)(['"]?)[^\n]*\n"#).expect("a regex"));
    let mut out = String::new();
    let mut rest = command;
    while let Some(c) = START.captures(rest) {
        let whole = c.get(0).expect("the match");
        if c[1] != c[3] {
            out.push_str(&rest[..whole.end()]);
            rest = &rest[whole.end()..];
            continue;
        }
        let word = c[2].to_string();
        let body = &rest[whole.end()..];
        // The body ends at a line holding the word alone (indented, with
        // trailing blanks), at the end or before a line break.
        let mut end = None;
        let mut at = 0;
        while at <= body.len() {
            let line_end = body[at..].find('\n').map_or(body.len(), |i| at + i);
            let line = &body[at..line_end];
            if line.trim_start().trim_end_matches([' ', '\t']) == word && at > 0 {
                end = Some(line_end);
                break;
            }
            if line_end >= body.len() {
                break;
            }
            at = line_end + 1;
        }
        match end {
            Some(e) => {
                out.push_str(&rest[..whole.start()]);
                rest = &body[e..];
            }
            None => {
                out.push_str(&rest[..whole.end()]);
                rest = body;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The milestones a command would reach.
pub fn milestone_kinds(command: &str) -> Vec<&'static str> {
    let commands = without_heredocs(command);
    KINDS
        .iter()
        .filter(|(_, re)| re.is_match(&commands))
        .map(|(k, _)| *k)
        .collect()
}

/// A command that never ran says nothing about a commit or a push in it:
/// one a hook blocked, one you declined, or one the shell couldn't parse.
static NOTHING_RAN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?m)^PreToolUse:\S+ hook error|doesn't want to proceed|^\(eval\):\d+: (?:unmatched|parse error)|^(?:zsh|bash|sh): (?:-c: )?(?:line \d+: )?(?:syntax error|parse error)").expect("a regex")
});

/// The ways `milestone_line` begins a failure's line.
const FAILURES: [&str; 3] = [
    "a commit failed: ",
    "a push failed: ",
    "opening a PR failed: ",
];

/// Whether a line `milestone_line` gave says a milestone failed, by how it
/// begins, never by what a commit's subject or a branch's name holds.
pub fn is_failure(line: &str) -> bool {
    FAILURES.iter().any(|f| line.starts_with(f))
}

/// What a milestone's result says, as a line, or nothing worth saying.
pub fn milestone_line(kind: &str, output: &str, failed: bool) -> Option<String> {
    static COMMIT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"\[([^\]\s]+)(?: \(root-commit\))? ([0-9a-f]{7,})\] (.+)").expect("a regex")
    });
    static TO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^To (\S+)").expect("a regex"));
    static BRANCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"->\s+(\S+)").expect("a regex"));
    static PR: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"https://github\.com/\S+/pull/(\d+)").expect("a regex"));
    if NOTHING_RAN.is_match(output) {
        return None;
    }
    let last_line: String = output
        .trim()
        .split('\n')
        .rfind(|l| !l.trim().is_empty())
        .map(|l| l.trim().chars().take(200).collect())
        .unwrap_or_default();
    match kind {
        "commit" => match COMMIT.captures(output) {
            Some(c) => Some(format!("committed `{}` {} on `{}`", &c[2], &c[3], &c[1])),
            None => failed.then(|| format!("{}{last_line}", FAILURES[0])),
        },
        "push" => {
            // A push into a local repository (a test's throwaway one) is no
            // news, and neither is a quiet one, which says nothing of where
            // it went.
            let to = TO.captures(output).map(|c| c[1].to_string());
            if to.as_deref().is_some_and(|t| {
                t.starts_with('/')
                    || t.starts_with('.')
                    || t.starts_with('~')
                    || t.starts_with("file:")
            }) {
                return None;
            }
            if failed {
                return Some(format!("{}{last_line}", FAILURES[1]));
            }
            to.as_ref()?;
            Some(match BRANCH.captures(output) {
                Some(b) => format!("pushed `{}`", &b[1]),
                None => "pushed".into(),
            })
        }
        "pr" => match PR.captures(output) {
            Some(c) => Some(format!("opened [PR #{}]({})", &c[1], &c[0])),
            None => failed.then(|| format!("{}{last_line}", FAILURES[2])),
        },
        _ => None,
    }
}

/// Each tab's transcript as followed: the file, how far it was read, and
/// the milestone commands waiting on their results.
struct Seen {
    file: PathBuf,
    offset: u64,
    pending: HashMap<String, Vec<&'static str>>,
}

#[derive(Default)]
pub struct Memory {
    followed: HashMap<String, Seen>,
}

/// A tab's news goes in its thread at once. With `broadcast` it also goes
/// to the main DM, which notifies you: while you're away, as the same
/// message; while you're at the computer, as a line that waits until you're
/// away.
/// Each message is counted as sent, and as notifying once its line in the
/// main DM is posted.
pub async fn ping(
    cx: &Cx,
    tab: &Tab,
    line: &str,
    kind: Kind,
    broadcast: bool,
) -> anyhow::Result<()> {
    let home = home_of(cx, tab).await?;
    let target = Target {
        window: tab.id.clone(),
        index: tab.index,
        name: tab.name.clone(),
        thread: home.clone(),
    };
    let name = label(tab.index, &tab.name);
    cx.log(&format!(
        "ping: {name} {}",
        line.chars().take(80).collect::<String>()
    ));
    let away = cx.presence.away();
    let text = format!("🤖 {name}: {line}");
    let ts = post(
        cx,
        &text,
        home.as_deref(),
        PostOpts::broadcast(broadcast && away),
    )
    .await?;
    cx.state.borrow_mut().alerts.insert(ts, target.clone());
    cx.save();
    let sent = Sent {
        kind,
        tab: Some((tab.index, tab.name.clone())),
        autopilot: tab.autopilot,
        notified: false,
    };
    sending::record(cx, &sent);
    if broadcast && away {
        sending::record(
            cx,
            &Sent {
                notified: true,
                ..sent
            },
        );
    } else if broadcast {
        notify(
            cx,
            text,
            home,
            target,
            Vec::new(),
            "wait",
            Box::new(|_| true),
            sent,
        )
        .await?;
    }
    Ok(())
}

pub async fn milestones(cx: &Cx, tabs: &[Tab]) {
    for tab in tabs {
        let Some(file) = tab.transcript.clone() else {
            continue;
        };
        let size = std::fs::metadata(&file).map(|m| m.len()).ok();
        let lines = {
            let mut mem = cx.mem.borrow_mut();
            let followed = &mut mem.milestones.followed;
            let fresh = followed.get(&tab.id).is_none_or(|s| s.file != file);
            if fresh {
                // A transcript first seen (or a tab now on another one) is
                // followed from its end: what happened before is old news.
                followed.insert(
                    tab.id.clone(),
                    Seen {
                        file: file.clone(),
                        offset: size.unwrap_or(0),
                        pending: HashMap::new(),
                    },
                );
                continue;
            }
            let seen = followed.get_mut(&tab.id).expect("followed");
            let Some(size) = size else {
                continue;
            };
            if size < seen.offset {
                seen.offset = size;
            }
            if size == seen.offset {
                continue;
            }
            read_new(seen, size)
        };
        // A failure is a kind of its own, which notifies by default;
        // progress stays in the tab's thread.
        for line in lines {
            let kind = if is_failure(&line) {
                Kind::Failures
            } else {
                Kind::Milestones
            };
            let send = sending::send_now(cx, kind, tab.autopilot);
            if send == Send::Off {
                continue;
            }
            if let Err(e) = ping(cx, tab, &line, kind, send == Send::Notify).await {
                cx.log(&format!("ping failed: {e}"));
            }
        }
    }
}

/// The milestones in what a transcript gained since it was last read.
fn read_new(seen: &mut Seen, size: u64) -> Vec<String> {
    let read = (|| -> std::io::Result<Vec<u8>> {
        let mut f = std::fs::File::open(&seen.file)?;
        f.seek(SeekFrom::Start(seen.offset))?;
        let mut buf = Vec::new();
        f.take(size - seen.offset).read_to_end(&mut buf)?;
        Ok(buf)
    })();
    let Ok(buf) = read else {
        return Vec::new();
    };
    let end = buf.iter().rposition(|b| *b == b'\n').map_or(0, |at| at + 1);
    seen.offset += end as u64;
    let mut lines = Vec::new();
    for raw in String::from_utf8_lossy(&buf[..end]).split('\n') {
        let Ok(entry) = serde_json::from_str::<Value>(raw) else {
            continue;
        };
        let assistant = entry.get("type").and_then(Value::as_str) == Some("assistant");
        for part in entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let kind = part.get("type").and_then(Value::as_str);
            if assistant
                && kind == Some("tool_use")
                && part.get("name").and_then(Value::as_str) == Some("Bash")
            {
                let command = part
                    .pointer("/input/command")
                    .map(crate::util::js_string)
                    .unwrap_or_default();
                let kinds = milestone_kinds(&command);
                let id = part
                    .get("id")
                    .map(crate::util::js_string)
                    .unwrap_or_default();
                if !kinds.is_empty() {
                    seen.pending.insert(id, kinds);
                }
            }
            if kind == Some("tool_result") {
                let id = part
                    .get("tool_use_id")
                    .map(crate::util::js_string)
                    .unwrap_or_default();
                let Some(kinds) = seen.pending.remove(&id) else {
                    continue;
                };
                let output = match part.get("content") {
                    Some(Value::Array(items)) => items
                        .iter()
                        .map(|c| c.get("text").and_then(Value::as_str).unwrap_or(""))
                        .collect::<Vec<_>>()
                        .join("\n"),
                    Some(v) if !v.is_null() => crate::util::js_string(v),
                    _ => String::new(),
                };
                let failed = part.get("is_error").is_some_and(crate::util::truthy);
                lines.extend(
                    kinds
                        .iter()
                        .filter_map(|k| milestone_line(k, &output, failed)),
                );
            }
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_run_is_no_milestone_and_a_command_that_never_ran_isnt_a_failure() {
        assert!(milestone_kinds("npm test && node --test").is_empty());
        assert_eq!(
            milestone_kinds("git commit -m x && git push"),
            ["push", "commit"]
        );
        assert_eq!(
            milestone_line("push", "PreToolUse:Bash hook error: refused", true),
            None
        );
        assert_eq!(
            milestone_line("commit", "(eval):1: unmatched \"", true),
            None
        );
        assert_eq!(
            milestone_line("push", "zsh: parse error near `)'", true),
            None
        );
        assert_eq!(
            milestone_line(
                "commit",
                "The user doesn't want to proceed with this tool use.",
                true
            ),
            None
        );
    }

    #[test]
    fn milestones_count_gits_own_push_and_commit_not_a_subcommands_nor_mentions() {
        assert_eq!(
            milestone_kinds("git -C /w --no-pager push origin x"),
            ["push"]
        );
        assert!(milestone_kinds("git stash push -m wip").is_empty());
        assert!(milestone_kinds("echo 'then git push'").is_empty());
        assert_eq!(milestone_kinds("cd w; git commit -am fix"), ["commit"]);
        assert_eq!(milestone_kinds("x=$(git push 2>&1)"), ["push"]);
        assert_eq!(milestone_kinds("gh pr create --fill"), ["pr"]);
        assert!(
            milestone_kinds("cat > notes.md <<'EOF'\ngit push\ngh pr create\nEOF").is_empty(),
            "a heredoc's body is text"
        );
        assert_eq!(
            milestone_kinds("cat > n.md <<EOF\nx\nEOF\ngit push"),
            ["push"],
            "what follows a heredoc is commands again"
        );
    }

    #[test]
    fn a_commit_whose_subject_says_failed_is_no_failure() {
        let line = milestone_line("commit", "[main abc1234] Fix failed login\n", false).unwrap();
        assert!(!is_failure(&line), "{line}");
        let line = milestone_line("push", "! [rejected] main -> main", true).unwrap();
        assert!(is_failure(&line), "{line}");
    }

    #[test]
    fn milestone_lines_say_where_things_went() {
        assert_eq!(
            milestone_line(
                "commit",
                "[feature abc1234] Fix the login\n 1 file changed",
                false
            )
            .as_deref(),
            Some("committed `abc1234` Fix the login on `feature`")
        );
        assert_eq!(
            milestone_line(
                "push",
                "To github.com:o/r.git\n   1..2  feature -> feature",
                false
            )
            .as_deref(),
            Some("pushed `feature`")
        );
        assert_eq!(
            milestone_line("push", "To /tmp/repo.git\n x -> x", false),
            None,
            "a local push"
        );
        assert_eq!(milestone_line("push", "", false), None, "a quiet one");
        assert_eq!(
            milestone_line(
                "push",
                "To github.com:o/r.git\n ! [rejected]\nerror: failed to push",
                true
            )
            .as_deref(),
            Some("a push failed: error: failed to push")
        );
        assert_eq!(
            milestone_line("pr", "https://github.com/o/r/pull/12345\n", false).as_deref(),
            Some("opened [PR #12345](https://github.com/o/r/pull/12345)")
        );
    }
}
