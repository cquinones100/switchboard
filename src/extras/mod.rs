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

use crate::mission::ViewId;

/// The commands, each with its key in tmux.conf, or none for one in the
/// palette alone (`mux::keys::Action::Extra`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// "Rename the tab": a name typed in, empty for its own name back.
    RenameTab,
    /// M-0: a tab by its number, typed, for the tabs past 9.
    GoToTab,
    /// C-w: the next key h, j, k or l moves to the pane on that side.
    PaneNav,
    /// "Mission control: <view>", one for every view mission control has
    /// (`ViewId::BUILT_IN`, and each plugin's while M-p is open).
    Mission(ViewId),
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
    /// "Filter the sidebar to a repository".
    SidebarRepo,
    /// "Show every repository's tabs in the sidebar".
    SidebarAll,
}

impl Command {
    /// Every command, a "Mission control: <view>" for each built-in view in
    /// mission control's order among them.
    pub fn all() -> Vec<Command> {
        [Command::RenameTab, Command::GoToTab, Command::PaneNav]
            .into_iter()
            .chain(ViewId::BUILT_IN.into_iter().map(Command::Mission))
            .chain([
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
                Command::SidebarRepo,
                Command::SidebarAll,
            ])
            .collect()
    }

    /// Its name in keys.json.
    pub fn name(&self) -> String {
        match self {
            Command::RenameTab => "rename-tab".into(),
            Command::GoToTab => "go-to-tab".into(),
            Command::PaneNav => "pane-nav".into(),
            Command::Mission(ViewId::Plugin(name)) => format!("mission-plugin:{name}"),
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
            Command::SidebarRepo => "sidebar-repo".into(),
            Command::SidebarAll => "sidebar-all".into(),
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
        if let Some(plugin) = name.strip_prefix("mission-plugin:") {
            return Some(Command::Mission(ViewId::Plugin(plugin.to_string())));
        }
        Command::all().into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(&self) -> String {
        let words = match self {
            Command::Mission(view) => return view.about(),
            Command::RenameTab => "Rename the tab",
            Command::GoToTab => "Go to a tab by number, for tabs past 9",
            Command::PaneNav => {
                "Then h, j, k or l: move to the pane on that side, as C-w does in Vim"
            }
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
            Command::SidebarRepo => "Filter the sidebar to a repository",
            Command::SidebarAll => "Show every repository's tabs in the sidebar",
        };
        words.to_string()
    }

    /// More on what it does than `about`, for the palette's preview of the chosen command.
    pub fn detail(&self) -> &'static str {
        match self {
            Command::RenameTab => {
                "Typed in a box that starts from the name it has, its PR number kept in front. An empty name gives the tab its own name back."
            }
            Command::GoToTab => {
                "Its number typed in a box. A number past the last tab, or 0, is said to have no tab."
            }
            Command::PaneNav => {
                "C-w twice sends one C-w to the pane. In Vim, the diff and Notes, C-w is theirs; Settings' \"C-w then h, j, k or l moves between panes\" (ctrl_w_panes) off leaves it to every pane."
            }
            Command::Mission(view) => view.detail(),
            Command::Notifications => {
                "The Notifications window, each one whole, Enter opening its actions. With Settings' \"Notifications window\" (notice_list) off, the sidebar's cursor goes to the first one instead."
            }
            Command::Habits => {
                "Counted on this computer alone, never sent, while Settings' \"Suggestions from how you work\" (habits) is on. Shown in a box that j and k scroll and any other key closes."
            }
            Command::SendReply => {
                "Read from this tab's transcript and pasted, without Enter, into the Claude of a tab you pick, which comes to the front to add to it. Said when this tab runs no Claude or it hasn't replied."
            }
            Command::UpdateMain => {
                "Runs switchboard update-main in the pane's repository, which only fast-forwards and leaves a checkout with uncommitted changes as it is. What it did shows in a box."
            }
            Command::CheckSetup => {
                "Runs switchboard setup --check, asking nothing, and shows what each part would change in a box."
            }
            Command::SetupPart => {
                "Lists the parts switchboard setup --parts names and every part at once, Tab marking several, then asks whether to run them or only check them. What it printed shows in a box."
            }
            Command::CalendarTab => {
                "The tab the Calendar view opens in the background when its day is out of date, here in front: Claude in your home directory, reading the calendar through its Google Calendar connector."
            }
            Command::SlackTab => {
                "The tab the Slack view opens in the background when it is out of date, here in front, in your home directory. With Settings' \"Slack without a tab\" (slack_background) on, mission control looks without it."
            }
            Command::OpenPrNumber => {
                "Typed in a box that starts with the clipboard's PR; empty is this tab's PR, and a bare number is one of the pane's repository. Then the browser, or a new tab working on it (wtpr). Needs GitHub."
            }
            Command::OpenTicketId => {
                "Typed in a box that starts with the clipboard's ticket; empty is this tab's, and a bare number takes the team key of the branch's ticket. Then the browser, or a new tab running wt. Needs Linear."
            }
            Command::AwsLogin => {
                "Runs switchboard aws-login in a pane beside yours, logging in to the one [sso-session] in ~/.aws/config; the pane stays until a key, so its code can be read. Needs the AWS CLI."
            }
            Command::SidebarRepo => {
                "Picks one of the repositories your tabs are in, a worktree counting as its main clone. Every other repository's tabs go under Other repos at the bottom, folded to one line of their marks until Enter or a click opens it, and M-a and M-A pass them by. Kept across restarts. Settings' \"Sidebar repository filter\" (sidebar_repo_filter)."
            }
            Command::SidebarAll => {
                "Clears the sidebar's repository filter, every tab back in the group of what it is doing, as a click on the filter's chip does."
            }
            Command::Keys => {
                "Mission control's Keys view: the workspace's keys by their first tag, then the sidebar's, mission control's, the diff's and Notes', then those only through M-p; Enter presses a workspace key and ? finds one by pressing it. With UX 2.0, a board with a heatmap of the Alt keys pressed this week. Settings' \"Keys\" (view_keys) off hands M-/ to the pane."
            }
        }
    }

    /// tmux.conf's tags: `#github` and `#linear` take a command out of the
    /// palette while that service is off.
    pub fn tags(&self) -> &'static [&'static str] {
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
            Command::SidebarRepo | Command::SidebarAll => &["sidebar", "tabs"],
        }
    }

    /// The key tmux.conf bound it to, or `None` for one only in the palette.
    pub fn key(&self) -> Option<&'static str> {
        match self {
            Command::GoToTab => Some("M-0"),
            Command::PaneNav => Some("C-w"),
            Command::Keys => Some("M-/"),
            _ => None,
        }
    }

    /// Whether the palette lists it: `C-w` had no description in tmux.conf,
    /// since it does nothing alone.
    pub fn in_palette(&self) -> bool {
        match self {
            Command::PaneNav => false,
            Command::Mission(view) => view.shows(),
            Command::SidebarRepo | Command::SidebarAll => {
                crate::settings::is_on("sidebar_repo_filter")
            }
            _ => true,
        }
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
        for c in Command::all() {
            assert_eq!(Command::parse(&c.name()), Some(c.clone()), "{}", c.name());
        }
        let plugin = Command::Mission(ViewId::Plugin("deploys".into()));
        assert_eq!(plugin.name(), "mission-plugin:deploys");
        assert_eq!(Command::parse(&plugin.name()), Some(plugin));
        assert_eq!(Command::Mission(ViewId::Landing).name(), "mission-landing");
        assert_eq!(
            Command::parse("mission-merge-queues"),
            Some(Command::Mission(ViewId::Landing)),
            "the Merge queues view's old name opens the Landing view"
        );
        let keyed: Vec<_> = Command::all().iter().filter_map(|c| c.key()).collect();
        assert_eq!(keyed, ["M-0", "C-w", "M-/"]);
        assert!(!Command::PaneNav.in_palette());
    }

    #[test]
    fn views_are_named_as_mission_control_names_them() {
        assert_eq!(ViewId::WorkMap.slug(), "work-map");
        assert_eq!(ViewId::Prs.slug(), "prs");
        assert!(Command::Mission(ViewId::Prs).tags().contains(&"github"));
        assert!(Command::Mission(ViewId::Inbox).tags().contains(&"linear"));
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
