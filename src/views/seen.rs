//! When each link of a note was first seen there, and the order M-p's link
//! commands list links in: newest first, across the pane, what you sent its
//! Claude and the tab's note. A note's links are read as a link box opens
//! (nothing here watches a file): the first time a note is read its links
//! are untimed, since when they were written is unknown, and a link found
//! there later is timed by the note's last save, so one just added leads
//! the list; a link taken out of the note is forgotten, so one put back is
//! new again. Kept in memory alone, only the links the note holds now; a
//! new build reads each note afresh. New with the Rust version.

use std::collections::HashMap;

use super::shown::Shown;

/// Every note's links, by the note's name, each with when it was first
/// seen there (milliseconds since the epoch), none for those there when
/// the note was first read.
#[derive(Debug, Default)]
pub struct Seen {
    notes: HashMap<String, Vec<(String, Option<i64>)>>,
}

impl Seen {
    /// A note's links read, the note last saved at `saved`: the new ones
    /// are timed by it, unless the note is read for the first time, and
    /// those no longer in it are forgotten.
    pub fn note(&mut self, name: &str, links: &[Shown], saved: i64) {
        let before = self.notes.get(name);
        let now = links
            .iter()
            .map(|link| {
                let at = match before {
                    None => None,
                    Some(kept) => match kept.iter().find(|(url, _)| *url == link.url) {
                        Some((_, at)) => *at,
                        None => Some(saved),
                    },
                };
                (link.url.clone(), at)
            })
            .collect();
        self.notes.insert(name.to_string(), now);
    }

    /// When `url` was first seen in the note.
    pub fn at(&self, name: &str, url: &str) -> Option<i64> {
        self.notes
            .get(name)?
            .iter()
            .find(|(u, _)| u == url)
            .and_then(|(_, at)| *at)
    }
}

/// A list of links, each once, ordered newest first by `at`: the timed
/// ones, newest first, then the untimed in the order given (a screen's
/// bottom first, then a note's top first).
pub fn newest_first(links: Vec<Shown>, at: impl Fn(&str) -> Option<i64>) -> Vec<Shown> {
    let mut timed: Vec<(i64, Shown)> = links
        .into_iter()
        .map(|link| (at(&link.url).unwrap_or(i64::MIN), link))
        .collect();
    timed.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
    timed.into_iter().map(|(_, link)| link).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(url: &str) -> Shown {
        Shown {
            text: String::new(),
            url: url.into(),
        }
    }

    fn urls(links: &[Shown]) -> Vec<&str> {
        links.iter().map(|l| l.url.as_str()).collect()
    }

    #[test]
    fn a_notes_first_links_are_untimed_and_one_added_later_is_timed() {
        let mut seen = Seen::default();
        seen.note("N", &[link("https://a.example/1")], 10);
        assert_eq!(seen.at("N", "https://a.example/1"), None);
        seen.note(
            "N",
            &[link("https://a.example/1"), link("https://b.example/2")],
            20,
        );
        assert_eq!(seen.at("N", "https://a.example/1"), None);
        assert_eq!(seen.at("N", "https://b.example/2"), Some(20));
        seen.note("N", &[link("https://b.example/2")], 30);
        assert_eq!(seen.at("N", "https://b.example/2"), Some(20));
        assert_eq!(seen.at("Other", "https://b.example/2"), None);
    }

    #[test]
    fn a_link_taken_out_of_a_note_and_put_back_is_new_again() {
        let mut seen = Seen::default();
        seen.note("N", &[link("https://a.example/1")], 10);
        seen.note("N", &[], 20);
        assert_eq!(seen.at("N", "https://a.example/1"), None);
        seen.note("N", &[link("https://a.example/1")], 30);
        assert_eq!(seen.at("N", "https://a.example/1"), Some(30));
    }

    #[test]
    fn timed_links_lead_newest_first_and_the_untimed_keep_their_order() {
        // A screen's links, bottom first, then a prompt's, then a note's.
        let listed = vec![
            link("https://screen.example/bottom"),
            link("https://screen.example/top"),
            link("https://prompt.example/sent"),
            link("https://note.example/old"),
            link("https://note.example/added"),
        ];
        let at = |url: &str| match url {
            "https://screen.example/top" => Some(5),
            "https://prompt.example/sent" => Some(10),
            "https://note.example/added" => Some(20),
            _ => None,
        };
        assert_eq!(
            urls(&newest_first(listed, at)),
            vec![
                "https://note.example/added",
                "https://prompt.example/sent",
                "https://screen.example/top",
                "https://screen.example/bottom",
                "https://note.example/old",
            ]
        );
    }
}
