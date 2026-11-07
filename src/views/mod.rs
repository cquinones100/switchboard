//! Reading things beside your work: the branch's PR description (M-v), its
//! Linear ticket (M-i), any link (the link view), and search everything
//! (M-S-f), with M-r and M-l opening the PR and the ticket in the browser
//! and M-m saying whether the PR can merge.
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
//! - `pager`: less's part: keys, search, resize, links.
//! - `pr`: the branch's PR (pr-view.sh, pr-view.js's `prepare`).
//! - `ticket`: a Linear ticket (ticket-view.sh, pr-view.js's `ticket`).
//! - `image`: the PR or ticket as GitHub draws it, as pictures in a pane's
//!   place (pr-image.js).
//! - `page`: what a link holds (link-view.sh's `show`, lib/link-view.js).
//! - `search`: search everything's sources (lib/search.js).
//! - `shown`: the links, images and videos a pane shows, for M-p's link
//!   commands (link-view.sh's `links_of` and `media_of`).
//! - `ready`: whether the PR can merge (pr-ready.sh).
//! - `serve`: the server's side: the keys, the boxes, the panes beside.

use std::path::PathBuf;
use std::process::ExitCode;

pub mod image;
pub mod markdown;
pub mod mermaid;
pub mod page;
pub mod pager;
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
}

impl Command {
    pub const ALL: [Command; 12] = [
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
            | Command::OpenMedia => None,
        }
    }
}

/// What the views' work off the server's task reports back.
#[derive(Debug)]
pub enum ViewsEvent {
    /// Something to say on a client: why a link couldn't be opened.
    Say { client: ClientId, text: String },
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
/// far and the sources still looking.
#[derive(Default)]
pub struct Views {
    pub(crate) searches: u64,
    pub(crate) found: std::collections::HashMap<ClientId, search::Running>,
}

/// The program's directory argument, else where it runs.
fn dir_arg(arg: Option<&String>) -> PathBuf {
    match arg {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => std::env::current_dir().unwrap_or_else(|_| crate::paths::home()),
    }
}

/// `switchboard view <file.md>`: a Markdown file, paged.
pub fn view_main(args: Vec<String>) -> ExitCode {
    let [file] = args.as_slice() else {
        eprintln!("Usage: switchboard view <file.md>");
        return ExitCode::from(2);
    };
    let content = match std::fs::read_to_string(file) {
        Ok(text) => pager::Content::Markdown {
            text,
            place: "the file in a Mermaid viewer".into(),
        },
        Err(e) => pager::Content::Failed(format!("Couldn't read {file}: {e}")),
    };
    pager::run(content, None, None)
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
