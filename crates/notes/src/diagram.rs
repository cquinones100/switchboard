//! A note's ```` ```mermaid ```` blocks drawn as their diagrams, as UX 2.0
//! boards 83 and 84 draw them: while the cursor is outside a block, the
//! diagram crates/ui/src/mermaid.rs draws takes the block's rows on the screen
//! (as a closed fold takes a section's), and while it is in the block, its
//! code shows, so editing is always on the code. A diagram wider than the
//! note is cut at its edge with a `›` on each row cut and a line saying
//! where all of it shows; a kind mermaid.rs can't draw keeps its code, with
//! a line saying so. The Format menu's "Show diagram code" shows a note's
//! blocks as code until "Show diagrams", and the notes_mermaid setting off
//! shows every block as code.
//!
//! Nothing here waits: a block's drawing is worked out once for its text
//! and the note's width and kept (`Diagrams`), so a frame only looks it up.

use std::collections::{HashMap, HashSet};

use unicode_segmentation::UnicodeSegmentation;

use super::render::Kind;
use switchboard_ui::draw::width;

/// The line under a diagram cut at the note's edge.
pub const CUT: &str = "› cut at the note's edge; ⋯ Show diagram code shows all of it";

/// A mermaid block: its fences' rows and the code between them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub start: usize,
    pub end: usize,
    pub source: String,
}

impl Block {
    pub fn holds(&self, row: usize) -> bool {
        (self.start..=self.end).contains(&row)
    }
}

/// The note's mermaid blocks, each closed by its fence (one left open is
/// still being written, and shows as code).
pub fn blocks<S: AsRef<str>>(lines: &[S], kinds: &[Kind]) -> Vec<Block> {
    let mut out = Vec::new();
    let mut row = 0;
    while row < kinds.len() {
        if kinds[row] != Kind::Fence {
            row += 1;
            continue;
        }
        let Some(end) = (row + 1..kinds.len()).find(|&r| kinds[r] == Kind::Fence) else {
            break;
        };
        let info = lines[row].as_ref().trim().trim_start_matches('`').trim();
        if info == "mermaid" {
            out.push(Block {
                start: row,
                end,
                source: lines[row + 1..end]
                    .iter()
                    .map(|l| l.as_ref())
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
        }
        row = end + 1;
    }
    out
}

/// A diagram's rows within `room` columns, each cut short with a `›` where
/// it is wider, and within `tall` rows with the line under it, so a diagram
/// taller than the note shows whole rows it can scroll to; and whether it
/// was cut either way.
pub fn fit(drawn: &str, room: usize, tall: usize) -> (Vec<(String, bool)>, bool) {
    let mut rows = fit_across(drawn, room);
    let mut cut = rows.iter().any(|(_, cut)| *cut);
    if cut || rows.len() > tall {
        let keep = tall.saturating_sub(1).max(1);
        if rows.len() > keep {
            rows.truncate(keep);
            cut = true;
        }
    }
    (rows, cut)
}

fn fit_across(drawn: &str, room: usize) -> Vec<(String, bool)> {
    drawn
        .lines()
        .map(|line| {
            if width(line) <= room {
                return (line.to_string(), false);
            }
            let mut kept = String::new();
            let mut used = 0;
            for g in line.graphemes(true) {
                let w = width(g);
                if used + w > room.saturating_sub(1) {
                    break;
                }
                kept.push_str(g);
                used += w;
            }
            (kept, true)
        })
        .collect()
}

/// The line under a block whose kind isn't drawn here.
pub fn not_drawn(source: &str) -> String {
    let head = source
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("%%") && *l != "---")
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or("");
    let kind = match head {
        "flowchart" | "graph" | "sequenceDiagram" | "" => {
            return "This diagram has nothing to draw yet; its code shows instead.".to_string();
        }
        "pie" => "Pie charts",
        "classDiagram" => "Class diagrams",
        k if k.starts_with("stateDiagram") => "State diagrams",
        "erDiagram" => "Entity relationship diagrams",
        "gantt" => "Gantt charts",
        "journey" => "User journeys",
        "gitGraph" => "Git graphs",
        "mindmap" => "Mind maps",
        "timeline" => "Timelines",
        _ => "Diagrams of this kind",
    };
    format!("{kind} aren't drawn in Notes; its code shows instead.")
}

/// The diagrams drawn, by their code and the width they were drawn for.
#[derive(Default)]
pub struct Diagrams {
    drawn: HashMap<(String, usize), Option<String>>,
    /// The drawings asked for since the last `sweep`.
    used: HashSet<(String, usize)>,
}

/// Drawings kept past what the last frame drew, before those it didn't
/// draw are let go (an edit leaves its block's old text behind).
const KEPT: usize = 64;

impl Diagrams {
    /// A block's drawing within `room` columns (a left-to-right flowchart
    /// wider than that drawn top to bottom), `None` for a kind that isn't
    /// drawn.
    pub fn get(&mut self, source: &str, room: usize) -> Option<&str> {
        let key = (source.to_string(), room);
        self.used.insert(key.clone());
        self.drawn
            .entry(key)
            .or_insert_with(|| switchboard_ui::mermaid::draw(source, room))
            .as_deref()
    }

    /// After a frame: past `KEPT`, only the drawings it asked for are kept,
    /// so every block of the note open stays drawn however many it holds.
    pub fn sweep(&mut self) {
        let used = std::mem::take(&mut self.used);
        if self.drawn.len() > KEPT {
            self.drawn.retain(|key, _| used.contains(key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::kinds;

    #[test]
    fn a_closed_mermaid_block_is_found_and_other_code_is_not() {
        let lines = [
            "# Note",
            "```mermaid",
            "graph TD",
            "  A --> B",
            "```",
            "```rust",
            "fn main() {}",
            "```",
            "```mermaid",
            "pie",
        ];
        let found = blocks(&lines, &kinds(&lines));
        assert_eq!(
            found,
            [Block {
                start: 1,
                end: 4,
                source: "graph TD\n  A --> B".into()
            }]
        );
        assert!(found[0].holds(4) && !found[0].holds(5));
    }

    #[test]
    fn a_wide_diagram_is_cut_with_a_mark_on_each_row_cut() {
        let (rows, cut) = fit("short\nmuch longer than that", 10, 30);
        assert_eq!(
            rows,
            [
                ("short".to_string(), false),
                ("much long".to_string(), true)
            ]
        );
        assert!(cut);
    }

    #[test]
    fn a_diagram_taller_than_the_note_is_cut_with_room_for_the_line_under_it() {
        let drawn = (0..10)
            .map(|i| format!("row {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (rows, cut) = fit(&drawn, 80, 6);
        assert_eq!(rows.len(), 5);
        assert!(cut);
        let (rows, cut) = fit(&drawn, 80, 10);
        assert_eq!(rows.len(), 10);
        assert!(!cut);
    }

    #[test]
    fn a_kind_not_drawn_says_so() {
        assert_eq!(
            not_drawn("pie title Build time\n  \"a\" : 1"),
            "Pie charts aren't drawn in Notes; its code shows instead."
        );
        assert!(not_drawn("").contains("nothing to draw yet"));
        assert!(not_drawn("stateDiagram-v2\n  [*] --> A").starts_with("State diagrams"));
    }

    #[test]
    fn a_drawing_is_kept_for_its_code_and_width() {
        let mut d = Diagrams::default();
        assert!(d.get("graph TD\n  A --> B", 80).is_some());
        assert_eq!(d.drawn.len(), 1);
        d.get("graph TD\n  A --> B", 80);
        assert_eq!(d.drawn.len(), 1);
        assert_eq!(d.get("pie\n  \"a\" : 1", 80), None);
        // Past the limit, a frame keeps what it drew, many as they are.
        for i in 0..70 {
            d.get(&format!("graph TD\n  A{i} --> B"), 80);
        }
        d.sweep();
        assert_eq!(d.drawn.len(), 72);
        for i in 0..70 {
            d.get(&format!("graph TD\n  A{i} --> B"), 80);
        }
        d.sweep();
        assert_eq!(d.drawn.len(), 70);
    }
}
