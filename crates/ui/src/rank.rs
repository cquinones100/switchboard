//! How what is typed in a list orders its rows: the palette's ranking
//! (palette.sh's `--rank`), taken from `switchboard`'s src/ui/palette.rs
//! with the menu's search (`menu::Filter`), which orders a menu's options
//! by it, so an app's menus are searched as the workspace's are.

use std::collections::HashMap;

/// What was typed, as it is kept and compared: lowercase, the spaces
/// between words one each, none at the ends.
pub fn normalize(query: &str) -> String {
    query
        .to_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Orders the rows that matched `query`, best first (palette.sh's
/// `--rank`): `rows` is each row's value (its action's name) and the text
/// it is matched on, in the list's order; `picks` is what was typed before
/// each pick and what was picked. A row picked after this same query comes
/// first, then one picked after a longer query this one starts, each tier
/// by how often; then by how each word typed fits the row's words (3 a
/// whole word, 2 its start, 1 inside it, summed); ties keep the list's order.
pub fn rank(query: &str, rows: &[(String, String)], picks: &[(String, String)]) -> Vec<usize> {
    let typed = normalize(query);
    let terms: Vec<&str> = typed.split(' ').filter(|t| !t.is_empty()).collect();
    let mut same: HashMap<&str, usize> = HashMap::new();
    let mut longer: HashMap<&str, usize> = HashMap::new();
    if !typed.is_empty() {
        for (past, value) in picks {
            if *past == typed {
                *same.entry(value.as_str()).or_default() += 1;
            } else if past.starts_with(&typed) {
                *longer.entry(value.as_str()).or_default() += 1;
            }
        }
    }
    let mut keyed: Vec<(usize, usize, usize, usize)> = rows
        .iter()
        .enumerate()
        .map(|(i, (value, text))| {
            let lowered = text.to_lowercase();
            let words: Vec<&str> = lowered
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            let score = terms
                .iter()
                .map(|term| {
                    words
                        .iter()
                        .map(|w| {
                            if w == term {
                                3
                            } else if w.starts_with(term) {
                                2
                            } else if w.contains(term) {
                                1
                            } else {
                                0
                            }
                        })
                        .max()
                        .unwrap_or(0)
                })
                .sum();
            (
                same.get(value.as_str()).copied().unwrap_or(0),
                longer.get(value.as_str()).copied().unwrap_or(0),
                score,
                i,
            )
        })
        .collect();
    keyed.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.1.cmp(&a.1))
            .then(b.2.cmp(&a.2))
            .then(a.3.cmp(&b.3))
    });
    keyed.into_iter().map(|(_, _, _, i)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(v, t)| (v.to_string(), t.to_string()))
            .collect()
    }

    #[test]
    fn whole_words_rank_over_starts_and_insides() {
        let list = rows(&[
            ("a", "Previous tab #tabs"),
            ("b", "Open the PR #github #pr"),
            ("c", "Split pane to the right #panes"),
        ]);
        assert_eq!(rank("pr", &list, &[]), vec![1, 0, 2]);
        assert_eq!(
            rank("", &list, &[]),
            vec![0, 1, 2],
            "nothing typed keeps the order"
        );
    }

    #[test]
    fn what_was_picked_after_the_same_words_comes_first() {
        let list = rows(&[
            ("split-left", "Split pane to the left"),
            ("split-right", "Split pane to the right"),
        ]);
        let picks = vec![
            ("split".to_string(), "split-right".to_string()),
            ("spl".to_string(), "split-left".to_string()),
            ("spl".to_string(), "split-left".to_string()),
        ];
        assert_eq!(rank("Split", &list, &picks), vec![1, 0]);
        assert_eq!(
            rank("sp", &list, &picks),
            vec![0, 1],
            "two picks after longer words beat one"
        );
    }
}
