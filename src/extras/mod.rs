//! The rest of tmux.conf's keys and M-p's commands, the ones that belong to
//! no other part: renaming the tab (tab-name.zsh), mission control opened on
//! one of its views and the notifications (`sidebar.sh mission view:<name>`,
//! `sidebar.sh notices`), how you use Switchboard (habits-report.sh), Claude's
//! last reply sent to another tab (reply-send.sh), local main brought up to
//! date (update-main.sh), Switchboard's setup checked or run a part at a time
//! (repo-update.sh's `setup-check` and `setup-part`), the Calendar and Slack
//! tabs (loop-tab.sh), a PR or ticket opened by number (pr-open.zsh,
//! ticket-open.zsh), the AWS login (aws-login.sh), M-0 (tab-goto.sh),
//! `C-w` then h, j, k or l between panes (tmux.conf's `window-nav` table),
//! and M-/, mission control's Keys view (`sidebar.sh mission view:keys`).
//!
//! What each does is said by its `Command`, as the other parts' are, so the
//! palette lists them with tmux.conf's descriptions and tags; `serve` puts
//! them on the server, with the keys that change what a key does there
//! (Alt-arrows with pane_arrows off or the diff or Notes in front, the
//! characters Option types where it isn't Alt).
//!
//! - `text`: a box of lines to read, scrolled with j and k, for what a
//!   command has to show at length (the habits report, setup's output).
//! - `naming`: what a new tab is called, asked after M-t and M-n
//!   (tab-name.zsh).
//! - `serve`: the server's side.

mod naming;
mod serve;
pub mod text;

use std::sync::LazyLock;

use regex::Regex;

pub use serve::{Extras, KeyFate};

/// A mission control view M-p opens it on (tmux.conf's `view:<name>`
/// bindings), by the name the view shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MissionView {
    WorkMap,
    Calendar,
    Slack,
    Closed,
    Prs,
    Tickets,
    Inbox,
    Ci,
    Landing,
    Sessions,
    Usage,
    Resources,
}

impl MissionView {
    pub const ALL: [MissionView; 12] = [
        MissionView::WorkMap,
        MissionView::Calendar,
        MissionView::Slack,
        MissionView::Closed,
        MissionView::Prs,
        MissionView::Tickets,
        MissionView::Inbox,
        MissionView::Ci,
        MissionView::Landing,
        MissionView::Sessions,
        MissionView::Usage,
        MissionView::Resources,
    ];

    /// The view's title in mission control.
    pub fn title(self) -> &'static str {
        match self {
            MissionView::WorkMap => "Work map",
            MissionView::Calendar => "Calendar",
            MissionView::Slack => "Slack",
            MissionView::Closed => "Closed",
            MissionView::Prs => "PRs",
            MissionView::Tickets => "Tickets",
            MissionView::Inbox => "Inbox",
            MissionView::Ci => "CI",
            MissionView::Landing => "Landing",
            MissionView::Sessions => "Sessions",
            MissionView::Usage => "Usage",
            MissionView::Resources => "Resources",
        }
    }

    /// Its title lowercased with dashes for spaces, as mission control's
    /// `View::slug` and sidebar.sh's `view:<name>` name it.
    pub fn slug(self) -> String {
        self.title().to_lowercase().replace(' ', "-")
    }

    fn about(self) -> &'static str {
        match self {
            MissionView::WorkMap => {
                "Mission control: Work map, every piece of work in your repositories"
            }
            MissionView::Calendar => "Mission control: Calendar, today's meetings",
            MissionView::Slack => "Mission control: Slack, what is new for you on Slack",
            MissionView::Closed => "Mission control: Closed, the tabs closed last, to reopen",
            MissionView::Prs => "Mission control: PRs, your pull requests by where they stand",
            MissionView::Tickets => "Mission control: Tickets, your Linear issues by state",
            MissionView::Inbox => {
                "Mission control: Inbox, your unread Linear mentions, comments and assignments"
            }
            MissionView::Ci => "Mission control: CI, your pull requests with failing checks",
            MissionView::Landing => {
                "Mission control: Landing, what is landing in each repository, its landing queue or merge queue, and which tabs have them"
            }
            MissionView::Sessions => {
                "Mission control: Sessions, each Claude session's context and age"
            }
            MissionView::Usage => {
                "Mission control: Usage, today's Claude spend and the rate limits"
            }
            MissionView::Resources => {
                "Mission control: Resources, the CPU and memory each tab uses"
            }
        }
    }

    fn tags(self) -> &'static [&'static str] {
        match self {
            MissionView::WorkMap => &["mission", "worktree"],
            MissionView::Calendar => &["mission", "calendar"],
            MissionView::Slack => &["mission", "slack"],
            MissionView::Closed => &["mission", "tabs"],
            MissionView::Prs => &["mission", "github", "pr"],
            MissionView::Tickets => &["mission", "linear", "ticket"],
            MissionView::Inbox => &["mission", "linear", "inbox"],
            MissionView::Ci => &["mission", "github", "ci"],
            MissionView::Landing => &["mission", "queue", "landing"],
            MissionView::Sessions => &["mission", "claude", "context"],
            MissionView::Usage => &["mission", "claude", "usage", "cost"],
            MissionView::Resources => &["mission", "cpu", "memory", "processes"],
        }
    }
}

/// The commands, each with its key in tmux.conf, or none for one in the
/// palette alone (`mux::keys::Action::Extra`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// "Rename the tab": a name typed in, empty for its own name back.
    RenameTab,
    /// M-0: a tab by its number, typed, for the tabs past 9.
    GoToTab,
    /// C-w: the next key h, j, k or l moves to the pane on that side.
    PaneNav,
    /// "Mission control: <view>".
    Mission(MissionView),
    /// "Go to the notifications".
    Notifications,
    /// "How you use Switchboard".
    Habits,
    /// "Send Claude's last reply to another tab".
    SendReply,
    /// "Update local main to origin's".
    UpdateMain,
    /// "Check Switchboard's setup".
    CheckSetup,
    /// "Run a part of Switchboard's setup, or only check it".
    SetupPart,
    /// "Open a Calendar tab".
    CalendarTab,
    /// "Open a Slack tab".
    SlackTab,
    /// "Open a PR in the browser by number or URL".
    OpenPrNumber,
    /// "Open a Linear ticket in the browser by ID, number or URL".
    OpenTicketId,
    /// "Log in to AWS (aws sso login) in your Chrome profile".
    AwsLogin,
    /// M-/: mission control's Keys view, every key.
    Keys,
}

impl Command {
    pub const ALL: [Command; 27] = [
        Command::RenameTab,
        Command::GoToTab,
        Command::PaneNav,
        Command::Mission(MissionView::WorkMap),
        Command::Mission(MissionView::Calendar),
        Command::Mission(MissionView::Slack),
        Command::Mission(MissionView::Closed),
        Command::Mission(MissionView::Prs),
        Command::Mission(MissionView::Tickets),
        Command::Mission(MissionView::Inbox),
        Command::Mission(MissionView::Ci),
        Command::Mission(MissionView::Landing),
        Command::Mission(MissionView::Sessions),
        Command::Mission(MissionView::Usage),
        Command::Mission(MissionView::Resources),
        Command::Notifications,
        Command::Habits,
        Command::SendReply,
        Command::UpdateMain,
        Command::CheckSetup,
        Command::SetupPart,
        Command::CalendarTab,
        Command::SlackTab,
        Command::OpenPrNumber,
        Command::OpenTicketId,
        Command::AwsLogin,
        Command::Keys,
    ];

    /// Its name in keys.json.
    pub fn name(self) -> String {
        match self {
            Command::RenameTab => "rename-tab".into(),
            Command::GoToTab => "go-to-tab".into(),
            Command::PaneNav => "pane-nav".into(),
            Command::Mission(view) => format!("mission-{}", view.slug()),
            Command::Notifications => "notifications".into(),
            Command::Habits => "habits-report".into(),
            Command::SendReply => "send-reply".into(),
            Command::UpdateMain => "update-main".into(),
            Command::CheckSetup => "check-setup".into(),
            Command::SetupPart => "setup-part".into(),
            Command::CalendarTab => "calendar-tab".into(),
            Command::SlackTab => "slack-tab".into(),
            Command::OpenPrNumber => "open-pr-number".into(),
            Command::OpenTicketId => "open-ticket-id".into(),
            Command::AwsLogin => "aws-login".into(),
            Command::Keys => "keys-view".into(),
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        // The Merge queues view became the Landing view; a binding to its
        // old name in keys.json still opens it.
        let name = if name == "mission-merge-queues" {
            "mission-landing"
        } else {
            name
        };
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(self) -> &'static str {
        match self {
            Command::RenameTab => "Rename the tab",
            Command::GoToTab => "Go to a tab by number, for tabs past 9",
            Command::PaneNav => {
                "Then h, j, k or l: move to the pane on that side, as C-w does in Vim"
            }
            Command::Mission(view) => view.about(),
            Command::Notifications => {
                "Go to the notifications, in the sidebar or the Notifications view"
            }
            Command::Habits => {
                "How you use Switchboard: the keys, M-p commands and views you use most, and the suggestions now"
            }
            Command::SendReply => "Send Claude's last reply to another tab",
            Command::UpdateMain => "Update local main to origin's, without switching branches",
            Command::CheckSetup => {
                "Check Switchboard's setup: say what isn't in place, changing nothing"
            }
            Command::SetupPart => "Run a part of Switchboard's setup, or only check it",
            Command::CalendarTab => {
                "Open a Calendar tab that keeps today's events in view (/loop 30m /today)"
            }
            Command::SlackTab => {
                "Open a Slack tab that keeps what is new for you on Slack in view (/loop 5m /slack-updates)"
            }
            Command::OpenPrNumber => "Open a PR in the browser by number or URL",
            Command::OpenTicketId => "Open a Linear ticket in the browser by ID, number or URL",
            Command::AwsLogin => "Log in to AWS (aws sso login) in your Chrome profile",
            Command::Keys => "Every key, by what it is about, with / to filter",
        }
    }

    /// tmux.conf's tags: `#github` and `#linear` take a command out of the
    /// palette while that service is off.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::RenameTab => &["tabs", "name"],
            Command::GoToTab => &["tabs", "navigate"],
            Command::PaneNav => &["panes", "navigate"],
            Command::Mission(view) => view.tags(),
            Command::Notifications => &["sidebar", "notifications"],
            Command::Habits => &["help", "habits", "report"],
            Command::SendReply => &["claude", "reply", "tabs"],
            Command::UpdateMain => &["git", "main", "sync"],
            Command::CheckSetup | Command::SetupPart => &["switchboard", "setup"],
            Command::CalendarTab => &["calendar", "claude", "tabs", "new"],
            Command::SlackTab => &["slack", "claude", "tabs", "new"],
            Command::OpenPrNumber => &["github", "pr", "browser"],
            Command::OpenTicketId => &["linear", "ticket", "browser"],
            Command::AwsLogin => &["aws", "login", "sso"],
            Command::Keys => &["help", "keys"],
        }
    }

    /// The key tmux.conf bound it to, or `None` for one only in the palette.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Command::GoToTab => Some("M-0"),
            Command::PaneNav => Some("C-w"),
            Command::Keys => Some("M-/"),
            _ => None,
        }
    }

    /// Whether the palette lists it: `C-w` had no description in tmux.conf,
    /// since it does nothing alone.
    pub fn in_palette(self) -> bool {
        self != Command::PaneNav
    }
}

// ── What the keys read ──────────────────────────────────────────────────────

/// Whether a pane's program is Vim or Neovim, which takes `C-w` for its own
/// windows: tmux.conf's `is_vim`, `ps -o comm` matched against
/// `^(\S+/)?g?(view|n?vim?x?)(diff)?$`, here on the program's name alone.
pub fn is_vim(command: &str) -> bool {
    static VIM: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^(\S+/)?g?(view|n?vim?x?)(diff)?$").unwrap());
    VIM.is_match(&command.to_lowercase())
}

/// Whether a pane's program is the diff or Notes (`switchboard diff`,
/// `switchboard notes`, by their arguments), which take `C-w` and the
/// M-arrows to move between their own windows and ask for the pane beside
/// them at their edges, as Neovim with vim-tmux-navigator did when both ran
/// in it. Vim itself takes only `C-w` (`is_vim`): no navigator of
/// Switchboard's runs in it to hand the M-arrows back.
pub fn takes_pane_keys(args: &[String]) -> bool {
    let name = |a: &String| a.rsplit('/').next().unwrap_or(a).to_string();
    match args {
        [program, app, rest @ ..] if name(program) == "switchboard" => {
            app == "diff" || (app == "notes" && rest.first().is_none_or(|a| a != "away"))
        }
        _ => false,
    }
}

/// The characters Option+h, j, k and l type on a US layout where the
/// terminal doesn't send Option as Alt (tmux.conf's `˙ ∆ ˚ ¬` bindings).
pub const OPTION_CHARS: [char; 4] = ['˙', '∆', '˚', '¬'];

/// A PR asked for by "Open a PR in the browser" (pr-open.zsh): a GitHub
/// PR's link, or its number in the pane's repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PrTyped {
    Url { url: String, number: u64 },
    Number(u64),
}

/// What was typed for a PR: its URL (as it is) or its number, `#` before it
/// or not; `None` for anything else.
pub fn pr_typed(text: &str) -> Option<PrTyped> {
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^https://github\.com/[^/]+/[^/]+/pull/([0-9]+)([/?#].*)?$").unwrap()
    });
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#?([0-9]+)$").unwrap());
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(m) = URL.captures(&text) {
        return Some(PrTyped::Url {
            number: m[1].parse().ok()?,
            url: text,
        });
    }
    NUMBER
        .captures(&text)
        .and_then(|m| m[1].parse().ok())
        .map(PrTyped::Number)
}

/// A ticket asked for by "Open a Linear ticket in the browser"
/// (ticket-open.zsh): a Linear issue's link, an ID, or a number that takes
/// the team key of the ticket the branch names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TicketTyped {
    Url { url: String, id: String },
    Id(String),
}

/// What was typed for a ticket, read with `branch_ticket` (the ticket the
/// pane's branch names) for a bare number; what is wrong with it, in words,
/// when it is none of them.
pub fn ticket_typed(text: &str, branch_ticket: Option<&str>) -> Result<TicketTyped, String> {
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"^https://linear\.app/[^/]+/issue/([A-Za-z][A-Za-z0-9]*-[0-9]+)([/?#].*)?$")
            .unwrap()
    });
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9]*-[0-9]+$").unwrap());
    static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^#?([0-9]+)$").unwrap());
    let text: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    if let Some(m) = URL.captures(&text) {
        return Ok(TicketTyped::Url {
            id: m[1].to_uppercase(),
            url: text.clone(),
        });
    }
    if ID.is_match(&text) {
        return Ok(TicketTyped::Id(text.to_uppercase()));
    }
    if let Some(m) = NUMBER.captures(&text) {
        let number = &m[1];
        let Some(team) = branch_ticket
            .and_then(|t| t.split_once('-'))
            .map(|(k, _)| k)
        else {
            return Err(format!(
                "Ticket {number} needs a team key, and this tab's branch doesn't name one. Type it in full, like ABC-{number}."
            ));
        };
        return Ok(TicketTyped::Id(format!("{}-{number}", team.to_uppercase())));
    }
    Err(format!("Not a ticket ID, number or Linear URL: {text}"))
}

/// The tab number typed for M-0 (tab-goto.sh took digits alone).
pub fn tab_number(text: &str) -> Option<usize> {
    let text = text.trim();
    if text.is_empty() || !text.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    text.parse().ok().filter(|n| *n > 0)
}

/// The line the clipboard holds, when it is what a field asks for, to start
/// the field with, as pr-open.zsh and ticket-open.zsh did with pbpaste.
pub fn prefill(clipboard: Option<&str>, takes: impl Fn(&str) -> bool) -> String {
    let line: String = clipboard
        .and_then(|c| c.lines().next())
        .unwrap_or("")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if !line.is_empty() && takes(&line) {
        line
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_round_trip_and_only_three_have_keys() {
        for c in Command::ALL {
            assert_eq!(Command::parse(&c.name()), Some(c), "{}", c.name());
        }
        assert_eq!(
            Command::Mission(MissionView::Landing).name(),
            "mission-landing"
        );
        assert_eq!(
            Command::parse("mission-merge-queues"),
            Some(Command::Mission(MissionView::Landing)),
            "the Merge queues view's old name opens the Landing view"
        );
        let keyed: Vec<_> = Command::ALL.iter().filter_map(|c| c.key()).collect();
        assert_eq!(keyed, ["M-0", "C-w", "M-/"]);
        assert!(!Command::PaneNav.in_palette());
    }

    #[test]
    fn views_are_named_as_mission_control_names_them() {
        assert_eq!(MissionView::WorkMap.slug(), "work-map");
        assert_eq!(MissionView::Prs.slug(), "prs");
        assert!(
            Command::Mission(MissionView::Prs)
                .tags()
                .contains(&"github")
        );
        assert!(
            Command::Mission(MissionView::Inbox)
                .tags()
                .contains(&"linear")
        );
    }

    #[test]
    fn vim_is_known_by_its_program() {
        for vim in [
            "vim",
            "nvim",
            "vi",
            "view",
            "gvim",
            "vimdiff",
            "nvimdiff",
            "/usr/bin/nvim",
        ] {
            assert!(is_vim(vim), "{vim}");
        }
        for other in ["zsh", "claude", "node", "vimx-thing", "less", "nview2"] {
            assert!(!is_vim(other), "{other}");
        }
    }

    #[test]
    fn the_diff_and_notes_take_the_pane_keys() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(takes_pane_keys(&args(&[
            "/home/u/.local/bin/switchboard",
            "diff",
            "/r"
        ])));
        assert!(takes_pane_keys(&args(&["switchboard", "notes"])));
        // Notes' placeholder in its own tab, the views, a shell, and Vim
        // (which takes C-w alone) don't.
        for other in [
            &["switchboard", "notes", "away"][..],
            &["switchboard", "pr-view"],
            &["zsh"],
            &["diff", "a", "b"],
            &["nvim", "x"],
        ] {
            assert!(!takes_pane_keys(&args(other)), "{other:?}");
        }
    }

    #[test]
    fn a_pr_is_a_link_or_a_number() {
        assert_eq!(pr_typed(" #12345 "), Some(PrTyped::Number(12345)));
        assert_eq!(
            pr_typed("https://github.com/o/r/pull/12/files"),
            Some(PrTyped::Url {
                url: "https://github.com/o/r/pull/12/files".into(),
                number: 12
            })
        );
        assert_eq!(pr_typed("https://github.com/o/r/issues/12"), None);
        assert_eq!(pr_typed("twelve"), None);
    }

    #[test]
    fn a_ticket_is_a_link_an_id_or_a_number_in_the_branchs_team() {
        assert_eq!(
            ticket_typed("abc-12", None),
            Ok(TicketTyped::Id("ABC-12".into()))
        );
        assert_eq!(
            ticket_typed("#34", Some("ENG-1")),
            Ok(TicketTyped::Id("ENG-34".into()))
        );
        assert!(
            ticket_typed("34", None)
                .unwrap_err()
                .contains("like ABC-34")
        );
        assert_eq!(
            ticket_typed("https://linear.app/team/issue/abc-9/a-title", None),
            Ok(TicketTyped::Url {
                url: "https://linear.app/team/issue/abc-9/a-title".into(),
                id: "ABC-9".into()
            })
        );
        assert!(ticket_typed("what", None).unwrap_err().starts_with("Not a"));
    }

    #[test]
    fn tab_numbers_are_digits_alone() {
        assert_eq!(tab_number(" 12 "), Some(12));
        assert_eq!(tab_number("0"), None);
        assert_eq!(tab_number("1a"), None);
        assert_eq!(tab_number(""), None);
    }

    #[test]
    fn the_clipboard_fills_a_field_only_with_what_it_takes() {
        let pr = |t: &str| pr_typed(t).is_some();
        assert_eq!(prefill(Some("#12\nmore"), pr), "#12");
        assert_eq!(prefill(Some("some words"), pr), "");
        assert_eq!(prefill(None, pr), "");
    }
}
