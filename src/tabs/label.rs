//! What a tab is called, as tmux/tab-label.sh, tmux/path-facts.sh and
//! tmux/window-names.sh put it together: the name given to the tab, else
//! the Linear ticket its branch names and the ticket's title, once Linear
//! has confirmed there is one, else the name its directory gives
//! (`color::window_name`); with `#<number> ` in front when the branch has a
//! pull request. The number goes first because it is the short, stable
//! half, so the tabs can be scanned by number.
//!
//! Nothing here reads a file or asks anything: the rules alone, so each can
//! be tested on its own.

/// A pane as window-names.sh sorted them when picking a tab's directory.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneKind {
    /// The tab's own work.
    Work,
    /// The notes pane moved beside it (`@notes_pane`), which runs in your
    /// home directory and isn't the tab's work.
    Notes,
    /// A sidebar (`@sidebar`), never the tab's.
    Sidebar,
}

/// Which pane a tab is named from, among its panes in order, each with
/// whether it is the active one: the active pane when it is the tab's work,
/// else its first pane that is, and only in a tab with nothing else, a notes
/// pane (the Notes tab itself). A sidebar never names a tab.
pub fn naming_pane(panes: &[(PaneKind, bool)]) -> Option<usize> {
    let work = |want_active: bool| {
        panes
            .iter()
            .position(|&(kind, active)| kind == PaneKind::Work && (active || !want_active))
    };
    work(true)
        .or_else(|| work(false))
        .or_else(|| panes.iter().position(|&(kind, _)| kind == PaneKind::Notes))
}

/// The label path-facts.sh gave a directory when no name is set: the ticket
/// and its title (`ENG-42: Fix the login`) when Linear knows the ticket,
/// else the directory's own name. Tabs become spaces, since the scripts
/// kept their fields apart with them and a later part may again.
pub fn facts_label(dir_name: &str, ticket: Option<(&str, &str)>) -> String {
    let label = match ticket {
        Some((id, title)) if !title.is_empty() => format!("{id}: {title}"),
        _ => dir_name.to_string(),
    };
    label.replace('\t', " ")
}

/// The tab's label: its own name when it has one, else `facts`, with the
/// PR's number in front.
pub fn label(own: Option<&str>, facts: &str, pr: Option<u64>) -> String {
    let name = own.filter(|n| !n.is_empty()).unwrap_or(facts);
    match pr {
        Some(n) => format!("#{n} {name}"),
        None => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::PaneKind::*;
    use super::*;

    #[test]
    fn a_given_name_wins_and_the_pr_goes_first() {
        assert_eq!(
            label(Some("Shaping: tabs"), "app:feature", None),
            "Shaping: tabs"
        );
        assert_eq!(label(Some(""), "app:feature", None), "app:feature");
        assert_eq!(label(None, "app:feature", Some(12)), "#12 app:feature");
        assert_eq!(label(Some("Mine"), "app", Some(7)), "#7 Mine");
        // A # of a name's own is kept as it is: nothing reads it as a format.
        assert_eq!(label(Some("#{x} ##"), "", None), "#{x} ##");
    }

    #[test]
    fn a_ticket_names_the_tab_once_linear_confirms_it() {
        assert_eq!(
            facts_label("app:eng-42-fix", Some(("ENG-42", "Fix the login"))),
            "ENG-42: Fix the login"
        );
        // Not known yet, or no such ticket: the directory's name.
        assert_eq!(
            facts_label("app:eng-42-fix", Some(("ENG-42", ""))),
            "app:eng-42-fix"
        );
        assert_eq!(facts_label("app:eng-42-fix", None), "app:eng-42-fix");
        assert_eq!(facts_label("a\tb", None), "a b");
        assert_eq!(
            facts_label("x", Some(("ENG-1", "Tabs\tin it"))),
            "ENG-1: Tabs in it"
        );
    }

    #[test]
    fn the_pane_a_tab_is_named_from() {
        assert_eq!(naming_pane(&[(Work, false), (Work, true)]), Some(1));
        // The sidebar's cursor on the tab makes it active; the tab still
        // goes by its own work.
        assert_eq!(
            naming_pane(&[(Sidebar, true), (Work, false), (Work, false)]),
            Some(1)
        );
        // Notes moved beside the work doesn't name the tab, even focused.
        assert_eq!(naming_pane(&[(Work, false), (Notes, true)]), Some(0));
        // The Notes tab itself does go by it.
        assert_eq!(naming_pane(&[(Sidebar, false), (Notes, true)]), Some(1));
        assert_eq!(naming_pane(&[(Sidebar, true)]), None);
        assert_eq!(naming_pane(&[]), None);
    }
}
