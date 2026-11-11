//! Reading things beside your work: the branch's PR description (M-v), its
//! Linear ticket (M-i), any link (the link view), and search everything
//! (M-S-f), with M-r and M-l opening the PR and the ticket in the browser
//! and M-m saying whether the PR can merge; and M-p's commands for the
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
//! - `mermaid`: flowcharts and sequence diagrams drawn as text.
//! - `pictures`: the pictures and Mermaid diagrams a view draws as
//!   images, where the terminal can show them.
//! - `pager`: less's part: keys, search, resize, links.
//! - `board`: the PR and the ticket as UX 2.0's boards draw them, a column
//!   and a panel beside it.
//! - `pr`: the branch's PR (pr-view.sh, pr-view.js's `prepare`).
//! - `ticket`: a Linear ticket (ticket-view.sh, pr-view.js's `ticket`).
//! - `image`: the PR or ticket as GitHub draws it, as pictures in a pane's
//!   place (pr-image.js).
//! - `page`: what a link holds (link-view.sh's `show`, lib/link-view.js).
//! - `search`: search everything's sources (lib/search.js).
//! - `shown`: the links, images and videos a pane shows, for M-p's link
//!   commands (link-view.sh's `links_of` and `media_of`).
//! - `given`: the links in what you sent each pane's Claude, pasted text
//!   included, which the link commands list too.
//! - `ready`: whether the PR can merge (pr-ready.sh).
//! - `serve`: the server's side: the keys, the boxes, the panes beside.

use std::path::PathBuf;
use std::process::ExitCode;

pub mod board;
pub mod given;
pub mod image;
pub mod markdown;
pub mod mermaid;
pub mod page;
pub mod pager;
pub mod pictures;
pub mod pr;
pub mod ready;
pub mod search;
mod serve;
pub mod shown;
pub mod ticket;

use crate::mux::server::ClientId;

/// What the views do from a key or the palette, an action of the
/// multiplexer's (`mux::keys::Action::View`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    /// M-v: the branch's PR description in a pane to the right, or closed.
    PrView,
    /// M-i: the branch's Linear ticket in a pane to the right, or closed.
    TicketView,
    /// M-r: the branch's PR in the browser.
    OpenPr,
    /// The repository on GitHub in the browser.
    OpenRepo,
    /// M-l: the branch's Linear ticket in the browser.
    OpenTicket,
    /// M-m: whether the branch's PR is ready to merge.
    PrReady,
    /// M-S-f: notes, Linear, GitHub and Slack searched at once.
    Search,
    /// A link, asked for (or chosen from the pane's), read beside the pane.
    ReadLink,
    /// The link read beside this pane, in the browser.
    OpenLinkRead,
    /// A link the pane shows, in the browser (or read beside it).
    OpenLink,
    /// A link the pane shows, copied (or opened in the browser).
    CopyLink,
    /// An image or video the pane shows, opened in its app (or shown in
    /// its folder).
    OpenMedia,
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
    pub const ALL: [Command; 19] = [
        Command::PrView,
        Command::TicketView,
        Command::OpenPr,
        Command::OpenRepo,
        Command::OpenTicket,
        Command::PrReady,
        Command::Search,
        Command::ReadLink,
        Command::OpenLinkRead,
        Command::OpenLink,
        Command::CopyLink,
        Command::OpenMedia,
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
            Command::PrReady => "pr-ready",
            Command::Search => "search-everything",
            Command::ReadLink => "read-link",
            Command::OpenLinkRead => "open-link-being-read",
            Command::OpenLink => "open-link",
            Command::CopyLink => "copy-link",
            Command::OpenMedia => "open-media",
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
            Command::PrView => {
                "View the branch's PR description in a pane to the right, or close it"
            }
            Command::TicketView => {
                "View the branch's Linear ticket, with its comments, in a pane to the right, or close it"
            }
            Command::OpenPr => "Open the branch's PR in the browser",
            Command::OpenRepo => "Open the repository in the browser",
            Command::OpenTicket => "Open the branch's Linear ticket in the browser",
            Command::PrReady => "Check whether the branch's PR is ready to merge",
            Command::Search => {
                "Search notes, Linear, GitHub and Slack at once, and read a result beside the pane"
            }
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
            Command::OpenMedia => {
                "Open an image or video from this pane in its app, or show it in its folder"
            }
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
                "The PR is found from the pane's branch, in a fork's upstream too, the copy last read showing while GitHub is asked again. With UX 2.0 on, it opens in this pane's place. With no PR, the view says why."
            }
            Command::TicketView => {
                "The ticket ID comes from the branch's name, else the tab's. The copy last read shows while Linear is asked again; with UX 2.0 on, it opens in this pane's place. With no ID found, a box says so."
            }
            Command::OpenPr => {
                "A branch with no PR yet opens its repository instead, and a box says why. The link opens in the Chrome profile browser.json gives GitHub, and what went wrong is said in a box."
            }
            Command::OpenRepo => {
                "The repository is the one origin names, whatever the branch, opened in the Chrome profile browser.json gives GitHub. When it can't open, a box says why: no git repository, no remote on GitHub or no token."
            }
            Command::OpenTicket => {
                "The ticket ID comes from the branch's name, else the tab's. With a workspace written in linear.json the link is made at once; without, Linear is asked where the ticket is."
            }
            Command::PrReady => {
                "The verdict is GitHub's own, so it agrees with the merge button, with the required checks, review, conflicts and auto-merge under it. The box offers to show the failing checks or have the tab's Claude resolve conflicts."
            }
            Command::Search => {
                "Each source's results show as it answers, Slack's through Claude's connector about 15 seconds after the rest; notes are read on this computer and sent nowhere. Ctrl-O opens a result in the browser instead."
            }
            Command::ReadLink => {
                "A PR or ticket reads as its view lays it out, a Notion page or Google Doc through Claude's connectors, any other page as its main text. It replaces a link read beside before; Ctrl-O opens it in the browser."
            }
            Command::OpenLinkRead => {
                "The link view anywhere in the tab counts, so it works from any pane there, opening in the Chrome profile browser.json gives the link's site. With no link being read in the tab, a box says so."
            }
            Command::OpenLink => {
                "The list is the links the pane shows and has scrolled past, newest first, or the note's links in the notes pane. Ctrl-O reads the chosen one beside the pane instead. With no links, a box says so."
            }
            Command::CopyLink => {
                "The list is the links the pane shows and has scrolled past, newest first, or the note's links in the notes pane. It goes to your terminal's clipboard and the computer's; Ctrl-O opens it instead."
            }
            Command::OpenMedia => {
                "The list is the picture and video files the pane names that exist, then its links ending in one, newest first. A link opens in the browser, and Ctrl-O shows a file in its folder. With none, a box says so."
            }
            Command::OpenMergeQueue => {
                "The queue is the one for the branch's PR's base branch, else for the repository's default branch when the branch has no PR. When it can't be found, a box says why."
            }
            Command::OpenProject => {
                "Linear is asked which project the branch's ticket is in, the ID taken from the branch's name, else the tab's. A ticket in no project, or no ticket at all, is said in a box."
            }
            Command::CopyTicketId => {
                "The ID is the first ticket-shaped part of the branch's name, else of the tab's name. A box says what was copied, or that there is no ticket."
            }
            Command::CopyTicketUrl => {
                "The link is made from the workspace written in linear.json, else asked of Linear. A box says what was copied, or that there is no ticket in the branch or tab name."
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
            Command::TicketView => &["linear", "ticket", "panes"],
            Command::OpenPr => &["github", "pr", "browser"],
            Command::OpenRepo => &["github", "repo", "browser"],
            Command::OpenTicket => &["linear", "ticket", "browser"],
            Command::PrReady => &["github", "pr", "merge", "checks"],
            Command::Search => &["search", "find", "notes"],
            Command::ReadLink => &["links", "read", "browser"],
            Command::OpenLinkRead => &["links", "browser"],
            Command::OpenLink => &["links", "browser"],
            Command::CopyLink => &["links", "copy"],
            Command::OpenMedia => &["links", "image", "video", "media"],
            Command::OpenMergeQueue => &["github", "merge", "browser"],
            Command::OpenProject => &["linear", "project", "browser"],
            Command::CopyTicketId | Command::CopyTicketUrl => &["linear", "ticket", "copy"],
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
            Command::PrReady => Some("M-m"),
            Command::Search => Some("M-F"),
            Command::OpenRepo
            | Command::ReadLink
            | Command::OpenLinkRead
            | Command::OpenLink
            | Command::CopyLink
            | Command::OpenMedia
            | Command::OpenMergeQueue
            | Command::OpenProject
            | Command::CopyTicketId
            | Command::CopyTicketUrl
            | Command::CopyPrNumber
            | Command::CopyPrUrl
            | Command::ClearImageCache => None,
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
    /// M-m's answer, or why there is none.
    Ready {
        client: ClientId,
        answer: Result<ready::Answer, String>,
    },
    /// A search's source answered.
    Found {
        client: ClientId,
        search: u64,
        source: search::Source,
        rows: Vec<search::Found>,
    },
}

/// What the server keeps for the views: each client's search, its rows so
/// far and the sources still looking; and the links in what you sent each
/// pane's Claude.
#[derive(Default)]
pub struct Views {
    pub(crate) searches: u64,
    pub(crate) found: std::collections::HashMap<ClientId, search::Running>,
    pub(crate) given: given::Given,
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

/// `switchboard pr-view [dir]`, and `pr-view --checks [dir]` for the PR's
/// failing checks, as M-m's "See the failing checks" showed them.
pub fn pr_view_main(args: Vec<String>) -> ExitCode {
    if args.first().is_some_and(|a| a == "--checks") {
        return ready::checks_main(dir_arg(args.get(1)));
    }
    pr::main(dir_arg(args.first()))
}

/// `switchboard ticket-view [dir|ID]`.
pub fn ticket_view_main(args: Vec<String>) -> ExitCode {
    let arg = args.first().cloned().unwrap_or_default();
    let id = ticket::id_of(&arg).filter(|_| !std::path::Path::new(&arg).is_dir());
    let id = match id {
        Some(id) => Some(id),
        None => ticket::id_for_dir(&dir_arg(args.first())),
    };
    ticket::main(id)
}

/// `switchboard link-view <url>`.
pub fn link_view_main(args: Vec<String>) -> ExitCode {
    let [url] = args.as_slice() else {
        eprintln!("Usage: switchboard link-view <url>");
        return ExitCode::from(2);
    };
    page::main(url)
}
