//! Tips (tmux/tip.sh): a small box in the top-right corner that teaches a
//! key the moment it would help, each tip once. Any key closes it, and that
//! key goes on as if the box hadn't been there, so nothing typed is lost; a
//! click closes it and goes no further; left alone, it closes after a few
//! seconds.
//!
//! What was shown is kept in `~/.local/state/switchboard/tips`, one name a
//! line, the file tip.sh kept, so a tip seen before the rewrite stays seen.
//! The server keeps the names in memory too (`Ui::tips_seen`), so a trigger
//! on a busy path (every tab switch) costs a lookup once its tip is shown.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;

use super::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit, Place};
use super::{Look, Outcome, Overlay, Step};
use crate::mux::input::{Key, Mouse};
use crate::{paths, util};

/// How long a tip stays up.
pub const SECONDS: u64 = 8;
const FOOTER: &str = "any key closes · tips are in Settings";

/// The file of tips shown.
pub fn seen_file() -> PathBuf {
    paths::state_dir().join("tips")
}

/// The tips shown, as the file lists them.
pub fn read_seen() -> BTreeSet<String> {
    util::read_text(seen_file())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

/// A tip's text, a line each, or `None` for a tip there isn't. `key` is the
/// key a `key-` tip names (tip.sh's TIP_KEY). The lines are tip.sh's
/// `text_of`, word for word; a tip whose key isn't ported yet still says it,
/// since its trigger comes with the key.
pub fn text_of(name: &str, key: Option<&str>) -> Option<Vec<String>> {
    // M-p, and the Commands button that opens it too while the
    // palette_button setting shows it: the key first, the click after.
    let palette = if crate::settings::is_on("palette_button") {
        "M-p (or click ⌘ Commands)"
    } else {
        "M-p"
    };
    let lines: Vec<String> = match name {
        "waiting" => vec![
            "A tab is waiting on you.".into(),
            String::new(),
            "M-a  jump to it, then to each tab".into(),
            "     that needs you after it".into(),
        ],
        "jump" => vec![
            "Lots of tabs?".into(),
            String::new(),
            "M-j  find one by typing its name".into(),
            "M-a  go to whichever needs you".into(),
        ],
        "in-place" => vec![
            "This took your pane's place.".into(),
            String::new(),
            "Quit it, or press the same key".into(),
            "again, and your pane comes back.".into(),
        ],
        "done" => vec![
            "A tab finished.".into(),
            String::new(),
            "Its mark clears once you look at it;".into(),
            "M-a  visits each finished tab".into(),
        ],
        "merged" => vec![
            "A tab's PR has merged.".into(),
            String::new(),
            "/queue-and-clean  cleans up after a merge".into(),
            "Enter in mission control's Work map".into(),
            "   can remove a merged worktree".into(),
        ],
        "pinned" => vec![
            "Pinned.".into(),
            String::new(),
            "It stays at the top of Needs you".into(),
            "until you press M-f in it again.".into(),
        ],
        "worktree" => vec![
            "Claude in a worktree.".into(),
            String::new(),
            "M-v  the PR's description".into(),
            "M-d  its diff, as GitHub shows it".into(),
            "M-m  whether it can merge".into(),
        ],
        "sidebar" => vec![
            "The sidebar.".into(),
            String::new(),
            "Enter  go to a tab     a  answer it".into(),
            "Space  peek at it      ?  every key".into(),
        ],
        "loop" => vec![
            "A tab is looping.".into(),
            String::new(),
            "It runs again on its own. Mission".into(),
            "control (M-Space) shows when, under Loops.".into(),
        ],
        "updates" => vec![
            "Updates.".into(),
            String::new(),
            "M-p's Update Switchboard looks any time,".into(),
            "and the update check is in Settings.".into(),
        ],
        "github_off" => vec![
            "GitHub is off in Settings.".into(),
            String::new(),
            "Its keys do nothing until it is on".into(),
            format!("again: {palette}, then Settings."),
        ],
        "linear_off" => vec![
            "Linear is off in Settings.".into(),
            String::new(),
            "Its keys do nothing until it is on".into(),
            format!("again: {palette}, then Settings."),
        ],
        // tip.sh pointed at `setup.sh terminal`, which set the terminal's
        // Option key; with no setup of the terminal now, it says where.
        "option-alt" => vec![
            "Option isn't acting as Alt here,".into(),
            "so the M- keys type letters instead.".into(),
            String::new(),
            "Set Option as Alt in the terminal's".into(),
            "settings (the guide's \"Option as Alt\").".into(),
        ],
        _ if name.starts_with("key-") => vec![
            format!("Next time, press {}", key.unwrap_or("its key")),
            String::new(),
            "for what you just picked from M-p.".into(),
        ],
        _ => return None,
    };
    Some(lines)
}

/// The tip for a key picked from the palette that has one of its own
/// (tip.sh's `key`): its name, one for each key, and the key as the palette
/// shows it, Shift spelled out (`M-L` is `key-sm-sl`, shown `M-S-L`).
pub fn key_tip(key: &str) -> (String, String) {
    let mut name = String::from("key-");
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            name.push('s');
            name.push(c.to_ascii_lowercase());
        } else if c.is_ascii_lowercase() || c.is_ascii_digit() {
            name.push(c);
        } else {
            name.push('-');
        }
    }
    (name, super::palette::shown_key(key))
}

pub struct Tip {
    name: String,
    lines: Vec<String>,
    until: Instant,
    close_lit: bool,
    drawn: Option<Drawn>,
}

impl Tip {
    /// The tip by that name, or `None` for one there isn't.
    pub fn new(name: &str, key: Option<&str>) -> Option<Tip> {
        Some(Tip {
            name: name.to_string(),
            lines: text_of(name, key)?,
            until: Instant::now() + Duration::from_secs(SECONDS),
            close_lit: false,
            drawn: None,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Overlay for Tip {
    fn kind(&self) -> &'static str {
        "tip"
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let lines: Vec<BoxLine> = self
            .lines
            .iter()
            .map(|l| BoxLine::text(format!(" {l}"), look.text()))
            .collect();
        let mut spec = BoxSpec::new("tip", &lines);
        spec.keys = Some(FOOTER);
        spec.place = Place::TopRight;
        spec.width = 0;
        spec.close_lit = self.close_lit;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    /// Any key closes it and goes on.
    fn key(&mut self, _key: Key, _look: &Look) -> Step {
        Step::Pass
    }

    /// A click or the wheel closes it, and goes no further, as tip.sh closed
    /// on any mouse report.
    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        match draw::gesture(&mouse) {
            Gesture::Click | Gesture::WheelUp | Gesture::WheelDown => {
                Step::Close(Outcome::Dismissed)
            }
            Gesture::Move if look.hover => {
                let over = self
                    .drawn
                    .as_ref()
                    .is_some_and(|d| d.hit(mouse.col, mouse.row) == Hit::Close);
                if over == self.close_lit {
                    Step::Same
                } else {
                    self.close_lit = over;
                    Step::Redraw
                }
            }
            _ => Step::Same,
        }
    }

    fn deadline(&self) -> Option<Instant> {
        Some(self.until)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mux::input::{KeyCode, Mods};
    use crate::ui::draw::tests::screen;
    use ratatui::layout::Rect;

    #[test]
    fn tips_have_their_text_and_others_none() {
        assert_eq!(text_of("pinned", None).unwrap()[0], "Pinned.");
        assert!(text_of("nonsense", None).is_none());
        assert_eq!(
            text_of("key-sm-sl", Some("M-S-L")).unwrap()[0],
            "Next time, press M-S-L"
        );
    }

    #[test]
    fn a_key_tip_is_named_for_its_key() {
        assert_eq!(key_tip("M-L"), ("key-sm-sl".into(), "M-S-L".into()));
        assert_eq!(key_tip("M-b d"), ("key-sm-b-d".into(), "M-b d".into()));
        assert_eq!(key_tip("M-["), ("key-sm--".into(), "M-[".into()));
    }

    #[test]
    fn it_sits_top_right_and_passes_any_key_on() {
        let mut tip = Tip::new("pinned", None).unwrap();
        let mut buf = Buffer::empty(Rect::new(0, 0, 80, 24));
        tip.draw(&mut buf, &Look::default());
        let d = tip.drawn.clone().unwrap();
        assert_eq!((d.outer.y, d.outer.right()), (0, 80));
        assert_eq!(
            d.outer.height,
            4 + 4,
            "its lines, a blank, the footer, edges"
        );
        let s = screen(&buf);
        assert!(s[0].contains("─ tip ─"), "{:?}", s[0]);
        assert!(s[1].contains("│ Pinned."));
        assert!(s[6].contains("any key closes · tips are in Settings"));
        assert_eq!(
            tip.key(Key::new(KeyCode::Char('j'), Mods::NONE), &Look::default()),
            Step::Pass
        );
    }
}
