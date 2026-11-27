//! Reading things beside your work: the branch's PR description (M-v), its
//! Linear ticket (M-i), any link (the link view), and search everything
//! (M-S-f), with M-r and M-l opening the PR and the ticket in the browser,
//! the PR view saying whether the PR can merge; and M-p's commands for the
//! branch's merge queue and Linear project in the browser (merge-queue.sh,
//! linear-project.sh), its ticket and PR copied (branch-copy.sh), and the
//! image views' cache cleared (image-cache-clear.sh).
//!
//! Under tmux each view was glow and less in a pane split off to the right
//! (tmux/pr-view.sh, ticket-view.sh, link-view.sh); here each is a program
//! of Switchboard's own, `switchboard pr-view`, `ticket-view`, `link-view`
//! and `view <file>`, run in a pane the server splits off the same way and
//! marks (`@pr_view`, `@ticket_view`, `@link_view`), so the key that opened
//! one closes it again. The programs draw Markdown as glow did and page it
//! as less did, so each is built and tested on its own.
//!
//! - `markdown`: Markdown laid out for a terminal, glow-style.json's look.
//! - `mermaid`: flowcharts (with their subgraphs) and sequence diagrams
//!   drawn as text, for the views and Notes.
//! - `pictures`: the pictures and Mermaid diagrams a view draws as
//!   images, where the terminal can show them.
//! - `pager`: less's part: keys, search, resize, links.
//! - `board`: the PR and the ticket as UX 2.0's boards draw them, a column
//!   and a panel beside it.
//! - `follow`: how the PR and ticket views read again what they show.
//! - `menu`: a board's menu in the pager (the PR view's `m`).
//! - `pr`: the branch's PR (pr-view.sh, pr-view.js's `prepare`).
//! - `ticket`: a Linear ticket (ticket-view.sh, pr-view.js's `ticket`),
//!   and what a tab's ticket is (`Ticket`): a Linear ticket or project,
//!   or the GitHub issue its branch was started for.
//! - `issue`: a GitHub issue, as the ticket view shows it.
//! - `project`: a Linear project, as the ticket view shows it.
//! - `image`: the PR or ticket as GitHub draws it, as pictures in a pane's
//!   place (pr-image.js).
//! - `page`: what a link holds (link-view.sh's `show`, lib/link-view.js).
//! - `search`: search everything's sources (lib/search.js).
//! - `search_box`: M-F's box, its query, source chips and results.
//! - `shown`: the links, images and videos a pane shows, for M-p's link
//!   commands (link-view.sh's `links_of` and `media_of`).
//! - `given`: the links in what you sent each pane's Claude, pasted text
//!   included, which the link commands list too, and in what it answered,
//!   each with when it was seen.
//! - `seen`: when each of a note's links was first seen there, and the
//!   link commands' order, newest first.
//! - `ready`: whether the PR can merge (pr-ready.sh), which the PR view
//!   says, and its failing checks.
//! - `serve`: the server's side: the keys, the boxes, the panes beside.

use std::path::PathBuf;
use std::process::ExitCode;

pub mod board;
pub mod conversation;
pub mod follow;
pub mod given;
pub mod image;
pub mod issue;
pub mod menu;
pub mod page;
pub mod pager;
pub mod pictures;
pub mod pr;
pub mod project;
pub mod ready;
pub mod search;
pub mod search_box;
pub mod seen;
mod serve;
pub(crate) use serve::{LINK_URL, LINK_VIEW, PR_VIEW, TAB_TICKET, TICKET_ID, TICKET_VIEW, exe};
pub mod shown;
pub mod ticket;

pub use switchboard_ui::{markdown, mermaid};

use crate::mux::server::ClientId;

/// What the views do from a key or the palette, an action of the
/// multiplexer's (`mux::keys::Action::View`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// M-v: the branch's PR description in Claude's place, or closed.
    PrView,
    /// M-i: the tab's ticket (its Linear ticket or project, or its GitHub
    /// issue) in Claude's place, or closed.
    TicketView,
    /// M-r: the branch's PR in the browser.
    OpenPr,
    /// The repository on GitHub in the browser.
    OpenRepo,
    /// M-l: the tab's ticket in the browser, Linear's or GitHub's.
    OpenTicket,
    /// The tab's ticket or project chosen from yours, kept on the tab.
    ChooseTicket,
    /// The branch's failing checks in a pane beside, from the PR view's menu.
    FailingChecks,
    /// `/resolve-conflicts` typed into the tab's Claude, from the PR view's
    /// menu.
    ResolveConflicts,
    /// M-S-f: the open tabs, notes, Linear, GitHub and Slack searched at once.
    Search,
    /// M-S-f with one source's prefix typed (`search::scope`).
    SearchTabs,
    SearchNotes,
    SearchLinear,
    SearchGitHub,
    SearchSlack,
    /// A link, asked for (or chosen from the pane's), read beside the pane.
    ReadLink,
    /// The link read beside this pane, in the browser.
    OpenLinkRead,
    /// A link the pane shows, in the browser (or read beside it).
    OpenLink,
    /// A link the pane shows, copied (or opened in the browser).
    CopyLink,
    /// A tab the pane's text names by number ("tab 15"), gone to (or its
    /// number copied).
    GoToMentionedTab,
    /// A tab the pane's text names by number, its number copied (or gone
    /// to).
    CopyMentionedTab,
    /// An image or video the pane shows, opened in its app (or shown in
    /// its folder).
    OpenMedia,
    /// A code file the pane names, read in the side panel with Vim's keys
    /// (`switchboard code`, src/code).
    OpenCode,
    /// The repository's merge queue for the branch's PR's base, else for
    /// its default branch, in the browser (merge-queue.sh).
    OpenMergeQueue,
    /// The Linear project of the branch's ticket in the browser
    /// (linear-project.sh).
    OpenProject,
    /// The branch's ticket ID, its link, its PR's number and its PR's
    /// link, copied (branch-copy.sh).
    CopyTicketId,
    CopyTicketUrl,
    CopyPrNumber,
    CopyPrUrl,
    /// What the PR and ticket image views keep, removed after a yes
    /// (image-cache-clear.sh; `image::cache`).
    ClearImageCache,
}

impl Command {
    /// Whether it reads the mission control view in front, when one is
    /// (`mission::view_text`), rather than the pane under it.
    pub fn reads_view_in_front(self) -> bool {
        matches!(
            self,
            Command::ReadLink
                | Command::OpenLink
                | Command::CopyLink
                | Command::GoToMentionedTab
                | Command::CopyMentionedTab
                | Command::OpenMedia
        )
    }

    pub const ALL: [Command; 29] = [
        Command::PrView,
        Command::TicketView,
        Command::OpenPr,
        Command::OpenRepo,
        Command::OpenTicket,
        Command::ChooseTicket,
        Command::FailingChecks,
        Command::ResolveConflicts,
        Command::Search,
        Command::SearchTabs,
        Command::SearchNotes,
        Command::SearchLinear,
        Command::SearchGitHub,
        Command::SearchSlack,
        Command::ReadLink,
        Command::OpenLinkRead,
        Command::OpenLink,
        Command::CopyLink,
        Command::GoToMentionedTab,
        Command::CopyMentionedTab,
        Command::OpenMedia,
        Command::OpenCode,
        Command::OpenMergeQueue,
        Command::OpenProject,
        Command::CopyTicketId,
        Command::CopyTicketUrl,
        Command::CopyPrNumber,
        Command::CopyPrUrl,
        Command::ClearImageCache,
    ];

    /// Its name in keys.json.
    pub fn name(self) -> &'static str {
        match self {
            Command::PrView => "pr-view",
            Command::TicketView => "ticket-view",
            Command::OpenPr => "open-pr",
            Command::OpenRepo => "open-repo",
            Command::OpenTicket => "open-ticket",
            Command::ChooseTicket => "choose-ticket",
            Command::FailingChecks => "failing-checks",
            Command::ResolveConflicts => "resolve-conflicts",
            Command::Search => "search-everything",
            Command::SearchTabs => "search-tabs",
            Command::SearchNotes => "search-notes",
            Command::SearchLinear => "search-linear",
            Command::SearchGitHub => "search-github",
            Command::SearchSlack => "search-slack",
            Command::ReadLink => "read-link",
            Command::OpenLinkRead => "open-link-being-read",
            Command::OpenLink => "open-link",
            Command::CopyLink => "copy-link",
            Command::GoToMentionedTab => "go-to-mentioned-tab",
            Command::CopyMentionedTab => "copy-mentioned-tab",
            Command::OpenMedia => "open-media",
            Command::OpenCode => "open-code",
            Command::OpenMergeQueue => "open-merge-queue",
            Command::OpenProject => "open-linear-project",
            Command::CopyTicketId => "copy-ticket-id",
            Command::CopyTicketUrl => "copy-ticket-url",
            Command::CopyPrNumber => "copy-pr-number",
            Command::CopyPrUrl => "copy-pr-url",
            Command::ClearImageCache => "clear-image-cache",
        }
    }

    pub fn parse(name: &str) -> Option<Command> {
        Command::ALL.into_iter().find(|c| c.name() == name)
    }

    /// What it does, as tmux.conf described its binding.
    pub fn about(self) -> &'static str {
        match self {
            Command::PrView => "View the branch's PR description in Claude's place, or close it",
            Command::TicketView => {
                "View the tab's ticket (its Linear ticket or project, or its GitHub issue), with its comments, in Claude's place, or close it"
            }
            Command::OpenPr => "Open the branch's PR in the browser",
            Command::OpenRepo => "Open the repository in the browser",
            Command::OpenTicket => {
                "Open the tab's ticket in the browser: its Linear ticket or project, or its GitHub issue"
            }
            Command::ChooseTicket => "Choose this tab's ticket or project…",
            Command::FailingChecks => "See the branch's failing checks in a pane beside",
            Command::ResolveConflicts => "Type /resolve-conflicts into this tab's Claude",
            Command::Search => {
                "Search the open tabs, notes, Linear, GitHub and Slack at once, and read a result beside the pane or go to the tab"
            }
            Command::SearchTabs => "Search the open tabs",
            Command::SearchNotes => "Search notes",
            Command::SearchLinear => "Search Linear",
            Command::SearchGitHub => "Search GitHub",
            Command::SearchSlack => "Search Slack",
            Command::ReadLink => {
                "Read a link in a pane beside this one, typed or one this pane shows"
            }
            Command::OpenLinkRead => "Open the link being read beside this pane in the browser",
            Command::OpenLink => {
                "Open a link from this pane in the browser, or read it in a pane beside"
            }
            Command::CopyLink => {
                "Copy a link from this pane to the clipboard, or open it in the browser"
            }
            Command::GoToMentionedTab => "Go to a tab this pane mentions, or copy its number",
            Command::CopyMentionedTab => "Copy the number of a tab this pane mentions, or go to it",
            Command::OpenMedia => {
                "Open an image or video from this pane in its app, or show it in its folder"
            }
            Command::OpenCode => "Open a file from this pane in the side panel",
            Command::OpenMergeQueue => "Open the repository's merge queue in the browser",
            Command::OpenProject => "Open the branch's Linear project in the browser",
            Command::CopyTicketId => "Copy the branch's ticket ID",
            Command::CopyTicketUrl => "Copy the branch's ticket URL",
            Command::CopyPrNumber => "Copy the branch's PR number",
            Command::CopyPrUrl => "Copy the branch's PR URL",
            Command::ClearImageCache => {
                "Clear the PR and ticket image cache: the pictures, stylesheet and pages the image views keep"
            }
        }
    }

    /// More on what it does than `about`, for the palette's preview of the chosen command.
    pub fn detail(self) -> &'static str {
        match self {
            Command::PrView => {
                "The PR is found from the pane's branch, in a fork's upstream too, the copy last read showing while GitHub is asked again, with whether it can merge in its head and m opening its menu. It opens in this pane's place, or in the side panel beside a narrow lone pane in place of what is there. With no PR, it shows the branch's CI on GitHub."
            }
            Command::TicketView => {
                "The ticket is one chosen for the tab, else the GitHub issue its branch was started for or the Linear ticket its branch names (repos.json's tickets saying which), else a ticket or project in a name you gave the tab. The copy last read shows while it is asked again, and it opens in this pane's place. With none found, you choose one of your Linear tickets and projects."
            }
            Command::OpenPr => {
                "A branch with no PR yet opens its repository instead, and a box says why. The link opens in the Chrome profile browser.json gives GitHub, and what went wrong is said in a box."
            }
            Command::OpenRepo => {
                "The repository is the one origin names, whatever the branch, opened in the Chrome profile browser.json gives GitHub. When it can't open, a box says why: no git repository, no remote on GitHub or no token."
            }
            Command::OpenTicket => {
                "The ticket is found as M-i finds it. A GitHub issue opens at once; with a workspace written in linear.json a Linear ticket's link is made at once, and without, Linear is asked where it is."
            }
            Command::ChooseTicket => {
                "Lists your Linear tickets and projects, most recently created first, and keeps the one chosen as the tab's ticket, which M-i and M-l then open, through a restart too. Esc changes nothing."
            }
            Command::FailingChecks => {
                "Each failing check of the branch's PR (or, with no PR, of the commit GitHub has for the branch) with its description and link, and a GitHub Actions job's last 25 lines of log. The PR view's menu offers it while a check fails."
            }
            Command::ResolveConflicts => {
                "Types it into the tab's Claude and moves you there to press Enter. The PR view's menu offers it while the PR has merge conflicts; a tab reviewing someone else's PR says the conflicts are its author's."
            }
            Command::Search => {
                "Each source's results show as it answers, Slack's through Claude's connector about 15 seconds after the rest; notes are read on this computer and sent nowhere. Ctrl-O opens a result in the browser instead."
            }
            Command::SearchTabs
            | Command::SearchNotes
            | Command::SearchLinear
            | Command::SearchGitHub
            | Command::SearchSlack => {
                "Opens M-F's search with this source's prefix typed (tab:, note:, linear:, gh:, slack:), so only it is asked. Prefixes can be typed in M-F too, more than one at a time."
            }
            Command::ReadLink => {
                "A PR or ticket reads as its view lays it out, a Notion page or Google Doc through Claude's connectors, any other page as its main text; over a mission control view, the list holds the view's links, the chosen row's first. It replaces a link read beside before; Ctrl-O opens it in the browser."
            }
            Command::OpenLinkRead => {
                "The link view anywhere in the tab counts, so it works from any pane there, opening in the Chrome profile browser.json gives the link's site. With no link being read in the tab, a box says so."
            }
            Command::OpenLink => {
                "The list is the links the pane shows and has scrolled past, newest first, or the note's links in the notes pane; over a mission control view, the view's, the chosen row's first. Ctrl-O reads the chosen one beside the pane instead. With no links, a box says so."
            }
            Command::CopyLink => {
                "The list is the links the pane shows and has scrolled past, newest first, or the note's links in the notes pane; over a mission control view, the view's, the chosen row's first. It goes to your terminal's clipboard and the computer's; Ctrl-O opens it instead."
            }
            Command::GoToMentionedTab => {
                "Lists the open tabs the pane's text names (tab 15, Tab #3, tabs 3, 5 and 7), newest first, this tab left out (over a mission control view, the view's text names), and goes to the one picked; with one named it goes there at once. Ctrl-O copies its number instead."
            }
            Command::CopyMentionedTab => {
                "Lists the open tabs the pane's text names (tab 15, Tab #3, tabs 3, 5 and 7), newest first, this tab left out (over a mission control view, the view's text names), and copies the picked one's number. Ctrl-O goes to it instead."
            }
            Command::OpenCode => {
                "The list is the files the pane names that exist, newest first, a path:line opening at that line; typing searches the repository's files too. The file opens read-only in the side panel beside a narrow lone pane, else in the pane's place, with Vim's keys and the diff's colors, following its changes on disk; q closes it. Off in Settings (code_view), it isn't listed."
            }
            Command::OpenMedia => {
                "The list is the picture and video files the pane names that exist, then its links ending in one, newest first; over a mission control view, the view's links to one and, on Slack, its threads' files. A link opens in the browser, and Ctrl-O shows a file in its folder. With none, a box says so."
            }
            Command::OpenMergeQueue => {
                "The queue is the one for the branch's PR's base branch, else for the repository's default branch when the branch has no PR. When it can't be found, a box says why."
            }
            Command::OpenProject => {
                "Linear is asked which project the branch's ticket is in, the ID taken from the branch's name, else the tab's. A ticket in no project, or no ticket at all, is said in a box."
            }
            Command::CopyTicketId => {
                "The ticket is found as M-i finds it: a GitHub issue is copied as its number (#12), a Linear ticket as its ID. A box says what was copied, or that there is no ticket."
            }
            Command::CopyTicketUrl => {
                "The ticket is found as M-i finds it. A GitHub issue's link is made at once; a Linear ticket's from the workspace written in linear.json, else asked of Linear. A box says what was copied, or that there is no ticket."
            }
            Command::CopyPrNumber => {
                "The PR is asked of GitHub by the pane's branch, a fork's upstream included. A box says what was copied, or that the branch has no pull request."
            }
            Command::CopyPrUrl => {
                "The PR is asked of GitHub by the pane's branch, a fork's upstream included, and its GitHub link taken. A box says what was copied, or that the branch has no pull request."
            }
            Command::ClearImageCache => {
                "After a yes, everything under the image views' caches goes, the text last read included, so the next image view fetches and draws again. With Settings' \"Cache PR and ticket images\" off, nothing is kept anyway."
            }
        }
    }

    /// tmux.conf's tags: `#github` and `#linear` take a command out of the
    /// palette while that service is off.
    pub fn tags(self) -> &'static [&'static str] {
        match self {
            Command::PrView => &["github", "pr", "panes"],
            Command::TicketView => &["linear", "github", "ticket", "panes"],
            Command::OpenPr => &["github", "pr", "browser"],
            Command::OpenRepo => &["github", "repo", "browser"],
            Command::OpenTicket => &["linear", "github", "ticket", "browser"],
            Command::ChooseTicket => &["linear", "github", "tabs", "ticket"],
            Command::FailingChecks => &["github", "pr", "checks"],
            Command::ResolveConflicts => &["github", "pr", "merge"],
            Command::Search => &["search", "find", "notes"],
            Command::SearchTabs => &["search", "find", "tabs"],
            Command::SearchNotes => &["search", "find", "notes"],
            Command::SearchLinear => &["linear", "search", "find"],
            Command::SearchGitHub => &["github", "search", "find"],
            Command::SearchSlack => &["search", "find", "slack"],
            Command::ReadLink => &["links", "read", "browser"],
            Command::OpenLinkRead => &["links", "browser"],
            Command::OpenLink => &["links", "browser"],
            Command::CopyLink => &["links", "copy"],
            Command::GoToMentionedTab => &["tabs", "links"],
            Command::CopyMentionedTab => &["tabs", "links", "copy"],
            Command::OpenMedia => &["links", "image", "video", "media"],
            Command::OpenCode => &["files", "navigate"],
            Command::OpenMergeQueue => &["github", "merge", "browser"],
            Command::OpenProject => &["linear", "project", "browser"],
            Command::CopyTicketId | Command::CopyTicketUrl => {
                &["linear", "github", "ticket", "copy"]
            }
            Command::CopyPrNumber | Command::CopyPrUrl => &["github", "pr", "copy"],
            Command::ClearImageCache => &["pr", "ticket", "images", "cache"],
        }
    }

    /// The key tmux.conf bound it to, or `None` for one only in the palette.
    pub fn key(self) -> Option<&'static str> {
        match self {
            Command::PrView => Some("M-v"),
            Command::TicketView => Some("M-i"),
            Command::OpenPr => Some("M-r"),
            Command::OpenTicket => Some("M-l"),
            Command::Search => Some("M-F"),
            Command::ReadLink => Some("M-u"),
            Command::OpenRepo
            | Command::SearchTabs
            | Command::SearchNotes
            | Command::SearchLinear
            | Command::SearchGitHub
            | Command::SearchSlack
            | Command::OpenLinkRead
            | Command::OpenLink
            | Command::CopyLink
            | Command::GoToMentionedTab
            | Command::CopyMentionedTab
            | Command::OpenMedia
            | Command::OpenCode
            | Command::OpenMergeQueue
            | Command::OpenProject
            | Command::ChooseTicket
            | Command::CopyTicketId
            | Command::CopyTicketUrl
            | Command::CopyPrNumber
            | Command::CopyPrUrl
            | Command::ClearImageCache
            | Command::FailingChecks
            | Command::ResolveConflicts => None,
        }
    }
}

/// What the views' work off the server's task reports back.
#[derive(Debug)]
pub enum ViewsEvent {
    /// Something to say on a client: why a link couldn't be opened.
    Say { client: ClientId, text: String },
    /// A link to open on a client, from a task that found it.
    Open { client: ClientId, url: String },
    /// Where a link is opening, read off the server's task: said until
    /// the browser comes to the front.
    Opening { client: ClientId, words: String },
    /// A link opened: whether the browser came to the front, or why it
    /// couldn't open; `words` is what was said meanwhile.
    Opened {
        client: ClientId,
        words: String,
        shown: Result<bool, String>,
    },
    /// Something to copy to a client's clipboard, and say it was.
    Copy { client: ClientId, text: String },
    /// M-F's box asks sources for a query (`search_box`), the Tabs
    /// source leaving out `pane`'s tab.
    Ask {
        client: ClientId,
        search: u64,
        query: String,
        sources: Vec<search::Source>,
        pane: Option<crate::mux::PaneId>,
    },
    /// M-F's chips set: the sources turned off, kept for the next M-F.
    Chips { off: Vec<search::Source> },
    /// A search's source answered.
    Found {
        client: ClientId,
        search: u64,
        source: search::Source,
        rows: Vec<search::Found>,
    },
}

/// What the server keeps for the views: the links in what you sent each
/// pane's Claude and in what it answered, and when each note's links were
/// first seen. A search's rows are its box's (`search_box`).
#[derive(Default)]
pub struct Views {
    pub(crate) given: given::Given,
    pub(crate) seen: seen::Seen,
}

/// The program's directory argument, else where it runs.
fn dir_arg(arg: Option<&String>) -> PathBuf {
    match arg {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::current_dir().unwrap_or_else(|_| crate::paths::home()),
    }
}

/// `switchboard view <file.md>`: a Markdown file, paged; `view --log
/// <file>`, a CI job's log kept by the CI view, paged from its end, as
/// mission control's "Read the failing log" showed it with `less +G`, or
/// `view --log +<line> <file>` from that line (its failing step's first);
/// `view +<line> <file>`, any file as plain text from that line, as the
/// diff's `gf` opens one it doesn't change.
pub fn view_main(args: Vec<String>) -> ExitCode {
    if let [flag, at, file] = args.as_slice()
        && flag == "--log"
        && let Some(line) = at.strip_prefix('+').and_then(|n| n.parse().ok())
    {
        // `readable` keeps the log's lines where they were, so the line
        // numbered in the CI view is the one shown.
        let content = match std::fs::read_to_string(file) {
            Ok(log) => pager::Content::Text {
                text: crate::sources::ciboard::readable(&log),
                line,
            },
            Err(e) => pager::Content::Failed(format!("Couldn't read {file}: {e}")),
        };
        return pager::run(content, None, None);
    }
    if let [at, file] = args.as_slice()
        && let Some(line) = at.strip_prefix('+').and_then(|n| n.parse().ok())
    {
        let content = match std::fs::read_to_string(file) {
            Ok(text) => pager::Content::Text { text, line },
            Err(e) => pager::Content::Failed(format!("Couldn't read {file}: {e}")),
        };
        return pager::run(content, None, None);
    }
    if let [flag, file] = args.as_slice()
        && flag == "--log"
    {
        let content = match std::fs::read_to_string(file) {
            Ok(log) => pager::Content::Log(crate::sources::ciboard::readable(&log)),
            Err(e) => pager::Content::Failed(format!("Couldn't read {file}: {e}")),
        };
        return pager::run(content, None, None);
    }
    let [file] = args.as_slice() else {
        eprintln!("Usage: switchboard view <file.md> | --log [+<line>] <file> | +<line> <file>");
        return ExitCode::from(2);
    };
    let content = match std::fs::read_to_string(file) {
        Ok(text) => pager::Content::Markdown {
            text,
            place: "the file in a Mermaid viewer".into(),
        },
        Err(e) => pager::Content::Failed(format!("Couldn't read {file}: {e}")),
    };
    pager::run_local(content)
}

/// `switchboard pr-view [dir]`, `pr-view --checks [dir]` for the PR's
/// failing checks, as the PR view's "See the failing checks" shows them,
/// and `pr-view --pr <owner>/<name>#<n>` for one PR by its number (a tab's
/// current PR from another branch, src/tabs/prs.rs); `--several` first
/// says the tab follows several PRs, for the menu's "Show another of this
/// tab's PRs".
pub fn pr_view_main(mut args: Vec<String>) -> ExitCode {
    if args.first().is_some_and(|a| a == "--several") {
        args.remove(0);
        pr::SEVERAL.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    if args.first().is_some_and(|a| a == "--checks") {
        return ready::checks_main(dir_arg(args.get(1)));
    }
    if args.first().is_some_and(|a| a == "--pr") {
        let pr = args.get(1).and_then(|a| crate::tabs::prs::TabPr::parse(a));
        let Some(crate::tabs::prs::TabPr {
            number,
            repo: Some(repo),
        }) = pr
        else {
            eprintln!(
                "Usage: switchboard pr-view [dir] | --checks [dir] | --pr <owner>/<name>#<n>"
            );
            return ExitCode::from(2);
        };
        return pr::main_numbered(repo, number);
    }
    pr::main(dir_arg(args.first()))
}

/// `switchboard ticket-view [dir|ID|project:<id>|owner/repo#n]`, or
/// `--ticket <ID|project:<id>|owner/repo#n>`: a Linear
/// ticket, a Linear project or a GitHub issue; a directory's ticket as its
/// branch and repos.json's `tickets` give it.
pub fn ticket_view_main(args: Vec<String>) -> ExitCode {
    // `--ticket <arg>` (what the server passes): that ticket, never a
    // directory of the same name.
    if let [flag, arg] = args.as_slice()
        && flag == "--ticket"
    {
        return match crate::tickets::Ticket::parse(arg) {
            Some(found) => ticket_of(found),
            None => ticket::main(None),
        };
    }
    let arg = args.first().cloned().unwrap_or_default();
    let given =
        crate::tickets::Ticket::parse(&arg).filter(|_| !std::path::Path::new(&arg).is_dir());
    let dir = dir_arg(args.first());
    let kept = crate::tickets::kept_known(&dir);
    match given.or_else(|| crate::tickets::of_dir(&dir, kept)) {
        Some(found) => ticket_of(found),
        None => ticket::none(crate::tickets::none_words(
            &dir,
            crate::tickets::none_in_branch_words(kept),
        )),
    }
}

/// The view of a ticket of each kind its sources keep.
fn ticket_of(found: crate::tickets::Ticket) -> ExitCode {
    match found {
        crate::tickets::Ticket::Issue { repo, number } => issue::main(repo, number),
        crate::tickets::Ticket::Project(id) => project::main(id),
        crate::tickets::Ticket::Linear(id) => ticket::main(Some(id)),
    }
}

/// `switchboard link-view <url>`.
pub fn link_view_main(args: Vec<String>) -> ExitCode {
    let [url] = args.as_slice() else {
        eprintln!("Usage: switchboard link-view <url>");
        return ExitCode::from(2);
    };
    page::main(url)
}
