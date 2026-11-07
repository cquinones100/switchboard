//! What a new tab is called, asked as tab-name.zsh asked it once M-t or M-n
//! had opened one (optional, Esc leaving it named after its directory).
//! Three ways, each on a key of its own: a name typed in (`n`, the box
//! M-p's "Rename the tab" opens), the ticket and its title
//! (`t`), or "Shaping: <ticket>" (`s`), the form `shape` gives a tab, which
//! `/wt` clears when it hands the tab over. Each ticket is the one the tab's
//! branch names when it names one, else asked for; the ticket's own branch
//! with `t` clears the name instead, since the tab shows its ticket and
//! title by itself there, kept current as Linear's title changes.
//!
//! The ticket and shaping names go in the tab's `@tab_name`, as tab-name.zsh
//! saved every name, so a skill that sets or clears it (`/shape`, `/wt`)
//! takes them over.

use std::path::PathBuf;

use super::{TicketTyped, ticket_typed};
use crate::integrations::linear;
use crate::mission::ask::Ask;
use crate::mux::TabId;
use crate::mux::server::{ClientId, Server};
use crate::ui::{Menu, MenuOption, Outcome};
use crate::views::ticket;
use crate::{settings, util};

impl Server {
    /// Asks what to call `tab`, a new tab opened in `dir`.
    pub(crate) fn ask_tab_name(&mut self, client: ClientId, tab: TabId, dir: PathBuf) {
        let mut options = vec![MenuOption::new("Type a name").key('n')];
        let linear = settings::is_on("linear");
        if linear {
            options.push(MenuOption::new("Its ticket and the ticket's title").key('t'));
        }
        options.push(MenuOption::new("Shaping a ticket").key('s'));
        let menu = Menu::new("Name the tab", options)
            .body("Optional: Esc leaves it named after its directory.");
        self.choose(client, menu, move |server, client, chose| {
            let shaping = match (chose, linear) {
                (Some(0), _) => return server.rename_tab(client, tab),
                (Some(1), true) => false,
                (Some(1), false) | (Some(2), true) => true,
                _ => return,
            };
            match ticket::id_for_dir(&dir) {
                Some(id) => server.name_after_ticket(client, tab, id, true, shaping),
                None => server.ask_ticket_for_name(client, tab, dir, shaping),
            }
        });
    }

    /// The ticket a tab is named after, when its branch names none.
    fn ask_ticket_for_name(&mut self, client: ClientId, tab: TabId, dir: PathBuf, shaping: bool) {
        let ask = Ask::new("Ticket", "Its ID, like ABC-123, or its Linear link.");
        self.open(
            client,
            Box::new(ask),
            Some(Box::new(move |server: &mut Server, client, outcome| {
                let Outcome::Picked(p) = outcome else {
                    return;
                };
                let branch = ticket::id_for_dir(&dir);
                match ticket_typed(&p.value, branch.as_deref()) {
                    Ok(TicketTyped::Id(id) | TicketTyped::Url { id, .. }) => {
                        server.name_after_ticket(client, tab, id, false, shaping)
                    }
                    Err(why) => server.say(Some(client), &why),
                }
            })),
        );
    }

    /// Names the tab "Shaping: <id>", or after the ticket and its title as
    /// Linear has it; the branch's own ticket (`branch`) clears the name,
    /// once Linear has confirmed the ticket, so the tab shows it by itself.
    fn name_after_ticket(
        &mut self,
        client: ClientId,
        tab: TabId,
        id: String,
        branch: bool,
        shaping: bool,
    ) {
        if shaping {
            self.set_tab_name(tab, &format!("Shaping: {id}"));
            return;
        }
        self.say(Some(client), "Reaching out to Linear...");
        let asked = id.clone();
        self.off_task(
            client,
            move || util::block_on(linear::ticket(&asked, "name the tab")),
            move |server, client, found| match found {
                Ok(Some(t)) if !t.title.is_empty() => {
                    server.close_overlays(client, "message");
                    let name = if branch {
                        String::new()
                    } else {
                        format!("{}: {}", t.identifier, t.title)
                    };
                    server.set_tab_name(tab, &name);
                }
                Ok(_) => server.say(
                    Some(client),
                    &format!("Linear has no title for {id}, so the tab was left as it was."),
                ),
                Err(e) => server.say(
                    Some(client),
                    &format!(
                        "Couldn't ask Linear about {id}, so the tab was left as it was.\n\n{e}"
                    ),
                ),
            },
        );
    }

    /// Sets the tab's `@tab_name` (empty takes it away) and names it again
    /// at once, rather than on the next tick.
    fn set_tab_name(&mut self, tab: TabId, name: &str) {
        let Some(t) = self.tabs.get_mut(&tab) else {
            return;
        };
        t.name.clear();
        if name.is_empty() {
            t.opts.remove("@tab_name");
        } else {
            t.opts.insert("@tab_name".into(), name.to_string());
        }
        self.name_tab(tab);
        self.dirty_tab(tab);
        self.mission_redraw();
    }
}
