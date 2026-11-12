//! The Find a key box (board 33 of the UX 2.0 canvas, new with the port):
//! `?` on the Keys view opens it, and every key pressed while it is up but
//! Enter and Esc is looked up rather than pressed. The key comes up as a
//! large keycap in the accent with what it does bold beside it, its tags,
//! whether it is the default or `keys.json`'s ("yours"), how often it was
//! pressed this week and its longer description; under a dashed rule, the
//! key looked up before it. Enter closes it showing the key in the Keys
//! view's list, at its row; Esc closes it.
//!
//! It looks a key up as the list would find it: the workspace's keys
//! pressed alone, the prefix and a key after it, then the apps' own keys
//! (the sidebar's, mission control's, the diff's, Notes'), with the app
//! named. What it looks in is what the server held as it opened (the
//! bindings, the Keys view's rows and the week's counts), so a key pressed
//! reads nothing; nothing it looks up is pressed.

use std::collections::HashMap;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::keymap;
use crate::items::Item;
use crate::mux::input::{Key, KeyCode, Mods, Mouse};
use crate::mux::keys::{Action, Bindings};
use crate::sidebar::text::wrap;
use crate::ui::draw::{self, BoxLine, BoxSpec, Drawn, Gesture, Hit, width};
use crate::ui::kit::{self, Tone};
use crate::ui::{Look, Outcome, Overlay, Step, Theme};

const PROMPT: &str = "Press a key, the way you would anywhere.";
/// The box's inside.
const INSIDE: u16 = 66;

/// A key looked up.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    /// The key as the list writes it (`M-S-H`, `M-b d`).
    pub key: String,
    /// What it does; `None` when nothing is bound to it.
    pub words: Option<String>,
    pub tags: String,
    /// The app whose own key it is.
    pub app: Option<String>,
    pub yours: bool,
    /// Pressed this week; `None` where nothing counts it.
    pub count: Option<u64>,
    pub detail: String,
    /// Its row in the Keys view, for Enter.
    pub row: Option<usize>,
}

/// What the box looks keys up in, as the server held it as it opened.
pub struct Lookup {
    pub bindings: Bindings,
    /// The Keys view's rows, unfiltered.
    pub rows: Vec<Item>,
    /// The week's presses by notation; `None` with habits off.
    pub counts: Option<HashMap<String, u64>>,
}

impl Lookup {
    /// A key pressed after the prefix (`after`) or alone.
    pub fn find(&self, key: Key, after: bool) -> Found {
        let notation = if after {
            format!("{} {}", self.bindings.prefix_key.notation(), key.notation())
        } else {
            key.notation()
        };
        let shown = crate::ui::palette::shown_key(&notation);
        let action = if after {
            self.bindings.prefix.get(&key)
        } else {
            self.bindings.root.get(&key)
        };
        // The habits count the keys pressed alone, not those after the
        // prefix (src/mux/server.rs), so a prefix key shows no count.
        let count = self
            .counts
            .as_ref()
            .filter(|_| !after)
            .map(|c| c.get(&notation).copied().unwrap_or(0));
        let app_rows = if after {
            Vec::new()
        } else {
            self.app_rows(key)
        };
        let named = |rows: &[usize]| -> String {
            rows.iter()
                .map(|&o| {
                    let i = &self.rows[o];
                    format!("{}: {}", keymap::app_of(i).unwrap_or(""), i.title)
                })
                .collect::<Vec<_>>()
                .join("; ")
        };
        if let Some(action) = action {
            // The row listing this very key, else its action's: the palette
            // lists a prefix key only when no key pressed alone does the
            // same (M-b x under M-w's row).
            let row = self
                .rows
                .iter()
                .position(|i| {
                    i.data["keys"]
                        .as_array()
                        .is_some_and(|ks| ks.iter().any(|k| k.as_str() == Some(&notation)))
                })
                .or_else(|| {
                    // M-1 to M-9 share M-1's row, as does any other key
                    // going to a tab by number (M-b 0, tab 10).
                    let listed = match action {
                        Action::SelectTab(_) => Action::SelectTab(1),
                        a => a.clone(),
                    };
                    self.rows
                        .iter()
                        .position(|i| keymap::action_of(i) == Some(listed.clone()))
                });
            let words = row
                .map(|r| self.rows[r].title.clone())
                .unwrap_or_else(|| action.about());
            let yours = if after {
                self.bindings.yours_prefix.contains(&key)
            } else {
                self.bindings.yours.contains(&key)
            };
            // An app may take the key for itself first where it is in
            // front.
            let mut detail = action.detail();
            if !app_rows.is_empty() {
                detail = format!(
                    "{detail} In front, an app takes it its own way: {}.",
                    named(&app_rows)
                )
                .trim_start()
                .to_string();
            }
            return Found {
                key: shown,
                words: Some(words),
                tags: tags_of(action),
                app: None,
                yours,
                count,
                detail,
                row,
            };
        }
        {
            // Every app that takes it, each its own way: the first shown,
            // the rest named in the description.
            if let Some(&r) = app_rows.first() {
                let item = &self.rows[r];
                let app = keymap::app_of(item).unwrap_or("").to_string();
                let others = named(&app_rows[1..]);
                let detail = if others.is_empty() {
                    format!("{app}'s own key: only {app} takes it, so it does nothing elsewhere.")
                } else {
                    format!(
                        "{app}'s own key. Also taken by {others}. Each app takes it its own way, and it does nothing elsewhere."
                    )
                };
                return Found {
                    key: shown,
                    words: Some(item.title.clone()),
                    tags: String::new(),
                    detail,
                    app: Some(app),
                    yours: false,
                    count: None,
                    row: Some(r),
                };
            }
        }
        Found {
            key: shown,
            words: None,
            tags: String::new(),
            app: None,
            yours: false,
            count: None,
            detail: String::new(),
            row: None,
        }
    }
}

impl Lookup {
    /// The rows of every app that takes a key pressed alone for itself, in
    /// the view's order.
    fn app_rows(&self, key: Key) -> Vec<usize> {
        let token = key.notation();
        // Easy mode's Cmd keys arrive as Ctrl+Alt+Shift (Ghostty).
        let cmd_label = crate::notes::easy::cmd_key(&key).and_then(|cmd| {
            crate::notes::easy::CMD_KEYS
                .iter()
                .find(|(_, c, ..)| *c == cmd)
                .map(|(_, _, label, _)| *label)
        });
        (0..self.rows.len())
            .filter(|&r| {
                let i = &self.rows[r];
                keymap::app_of(i).is_some()
                    && keymap::key_of(i)
                        .is_some_and(|k| Some(k) == cmd_label || label_keys(k).contains(&token))
            })
            .collect()
    }
}

/// The keys an app's label names, each as `Key::notation` writes it: its
/// tokens are keys that each do it (`h esc q ^c`, `1-9`), up to a word
/// that is no key (`↵ on a heading` names Enter alone), but a label that
/// starts with a leader (`Space b`) is one key after another, which no
/// single key presses, so it names none.
fn label_keys(label: &str) -> Vec<String> {
    let tokens: Vec<&str> = label.split([' ', ',']).filter(|t| !t.is_empty()).collect();
    if tokens.len() > 1 && tokens[0].eq_ignore_ascii_case("space") {
        return Vec::new();
    }
    let mut out = Vec::new();
    for t in tokens {
        match t {
            // A range of digits, `1-9` or `1-7`.
            t if t.len() == 3
                && t.as_bytes()[1] == b'-'
                && t.as_bytes()[0].is_ascii_digit()
                && t.as_bytes()[2].is_ascii_digit() =>
            {
                let (from, to) = (t.as_bytes()[0] - b'0', t.as_bytes()[2] - b'0');
                out.extend((from..=to).map(|n| n.to_string()));
            }
            "⇧↵" => out.push(canonical("S-Enter")),
            "⇧tab" => out.push(canonical("BTab")),
            "tab" => out.push(canonical("Tab")),
            "esc" => out.push("Escape".into()),
            "space" => out.push("Space".into()),
            "↵" => out.push("Enter".into()),
            t if t.starts_with('^') && t.chars().count() == 2 => out.push(format!("C-{}", &t[1..])),
            t if t.eq_ignore_ascii_case("M-space") => out.push("M-Space".into()),
            // A word that reads as no key starts the label's words about
            // where it works (`↵ on a heading`): the keys end there.
            t if Key::parse(&canonical(t)).is_none() => break,
            t => out.push(canonical(t)),
        }
    }
    // The arrows and Home and End, which the apps take wherever their
    // labels name the pair of letters for them (`j k`, `h j k l`, `g G`);
    // a letter alone (`h esc q`) means something else.
    let pairs = [
        (("j", "k"), ("Down", "Up")),
        (("h", "l"), ("Left", "Right")),
        (("g", "G"), ("Home", "End")),
    ];
    for ((a, b), (to_a, to_b)) in pairs {
        let has = |out: &[String], k: &str| out.iter().any(|x| x == k);
        if has(&out, a) && has(&out, b) {
            for key in [to_a, to_b] {
                if !has(&out, key) {
                    out.push(key.to_string());
                }
            }
        }
    }
    out
}

/// A key as a label spells it (`Ctrl-Shift-P`, `Ctrl-F`, `M-x`) in
/// `Key::notation`'s form (`C-S-p`, `C-f`, `M-x`), or as it is when it
/// reads as no key.
fn canonical(token: &str) -> String {
    let mut t = token
        .replace("Ctrl-", "C-")
        .replace("Shift-", "S-")
        .replace("Alt-", "M-");
    if t.starts_with("C-")
        && let Some(last) = t.pop()
    {
        t.push(last.to_ascii_lowercase());
    }
    Key::parse(&t).map_or_else(|| token.to_string(), |k| k.notation())
}

/// An action's tags as the palette writes them.
fn tags_of(action: &Action) -> String {
    action
        .tags()
        .iter()
        .map(|t| format!("#{t}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Which of the box's keycaps Tab or the mouse is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Cap {
    Show,
    Close,
}

pub struct FindKey {
    lookup: Lookup,
    caps: bool,
    /// The prefix was pressed: the next key is looked up after it.
    after_prefix: bool,
    now: Option<Found>,
    before: Option<Found>,
    focus: Option<Cap>,
    hover: Option<Cap>,
    drawn: Option<Drawn>,
    /// The keycaps' line and the columns each takes inside the box.
    cap_cols: Vec<(Cap, u16, u16)>,
    caps_line: usize,
}

impl FindKey {
    pub fn new(lookup: Lookup, caps: bool) -> FindKey {
        FindKey {
            lookup,
            caps,
            after_prefix: false,
            now: None,
            before: None,
            focus: None,
            hover: None,
            drawn: None,
            cap_cols: Vec::new(),
            caps_line: 0,
        }
    }

    /// The key looked up now.
    pub fn found(&self) -> Option<&Found> {
        self.now.as_ref()
    }

    /// Looks a key up, the one before it kept.
    fn look_up(&mut self, key: Key) {
        let found = if !self.after_prefix && key == self.lookup.bindings.prefix_key {
            self.after_prefix = true;
            Found {
                key: key.notation(),
                words: Some(
                    "The prefix: the key after it is looked up as one after the prefix".into(),
                ),
                tags: String::new(),
                app: None,
                yours: false,
                count: None,
                detail: String::new(),
                row: None,
            }
        } else {
            let found = self.lookup.find(key, self.after_prefix);
            self.after_prefix = false;
            found
        };
        if let Some(now) = self.now.take() {
            self.before = Some(now);
        }
        self.now = Some(found);
    }

    /// Enter: closes showing the key at its row, or does nothing before a
    /// key with a row.
    fn show(&self) -> Step {
        match self.now.as_ref().and_then(|f| f.row) {
            Some(row) => Step::Close(Outcome::Chose(row)),
            None => Step::Same,
        }
    }

    fn lines(&mut self, theme: &Theme) -> Vec<BoxLine> {
        let inside = INSIDE as usize - 2;
        let dim = Tone::Dim.style(theme);
        let mut out = vec![BoxLine::spans(vec![
            Span::raw(" "),
            Span::styled(PROMPT, dim),
        ])];
        if let Some(f) = &self.now {
            out.push(BoxLine::blank());
            out.extend(big_keycap(f, self.caps, theme, inside));
            if !f.detail.is_empty() {
                out.push(BoxLine::blank());
                for w in wrap(&f.detail, inside - 1) {
                    out.push(BoxLine::spans(vec![Span::raw(" "), Span::styled(w, dim)]));
                }
            }
            if let Some(b) = &self.before {
                out.push(BoxLine::spans(vec![
                    Span::raw(" "),
                    kit::head_rule(inside - 1, theme),
                ]));
                let words = match &b.words {
                    Some(w) => w.clone(),
                    None => "nothing bound; keys.json can bind it".into(),
                };
                out.push(BoxLine::spans(vec![
                    Span::raw(" "),
                    Span::styled("Before it, ", dim),
                    Span::styled(b.key.clone(), Tone::Accent.bold(theme)),
                    Span::styled(format!(": {words}"), dim),
                ]));
            }
        }
        out.push(BoxLine::spans(vec![
            Span::raw(" "),
            kit::rule(inside - 1, theme),
        ]));
        let mut caps_row = vec![
            Span::raw(" "),
            Span::styled("any key", Tone::Accent.style(theme)),
            Span::styled(" look it up  ", dim),
        ];
        self.cap_cols.clear();
        for (cap, key, what) in [
            (Cap::Show, "↵", "show it in the list"),
            (Cap::Close, "esc", "close"),
        ] {
            let lit = self.focus == Some(cap) || self.hover == Some(cap);
            let from = caps_row.iter().map(|s| width(&s.content)).sum::<usize>() as u16;
            caps_row.extend(kit::keycap(key, what, lit, self.caps, theme));
            let to = caps_row.iter().map(|s| width(&s.content)).sum::<usize>() as u16;
            self.cap_cols.push((cap, from, to.saturating_sub(1)));
        }
        self.caps_line = out.len();
        out.push(BoxLine::spans(caps_row).clickable());
        out
    }
}

/// The key as a keycap three rows tall in the accent, what it does bold
/// beside its middle row, and under that its tags, where it is from and
/// how often it was pressed this week.
fn big_keycap(f: &Found, caps: bool, theme: &Theme, inside: usize) -> Vec<BoxLine> {
    let accent = Tone::Accent.fg(theme);
    let cap_w = width(&f.key) + 6;
    let on_accent = Style::new().bg(accent);
    let pad = |s: Vec<Span<'static>>| {
        let mut row = vec![
            Span::raw(" "),
            Span::styled(" ".repeat(cap_w), on_accent),
            Span::raw("  "),
        ];
        row.extend(s);
        BoxLine::spans(kit::fit(row, inside, Style::new()))
    };
    let mut middle = vec![
        Span::raw(" "),
        Span::styled(
            kit::centered(&f.key, cap_w),
            on_accent
                .fg(Color::Indexed(232))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
    ];
    let words = match (&f.words, &f.app) {
        (Some(w), Some(app)) => format!("{app}: {w}"),
        (Some(w), None) => w.clone(),
        (None, _) => "Nothing bound".into(),
    };
    middle.push(Span::styled(words, Tone::Text.bold(theme)));
    let mut under: Vec<Span<'static>> = Vec::new();
    if f.words.is_some() && f.app.is_none() {
        if !f.tags.is_empty() {
            under.push(Span::styled(
                format!("{}  ", f.tags),
                Tone::Dim.style(theme),
            ));
        }
        if f.yours {
            under.extend(kit::chip("yours", Tone::Needs, caps, theme));
        } else {
            under.push(Span::styled("default  ", Tone::Dim.style(theme)));
        }
        if let Some(n) = f.count {
            let words = match n {
                0 => "not pressed this week".to_string(),
                n => format!("{n} this week"),
            };
            under.push(Span::styled(words, Tone::Text.style(theme)));
        }
    } else if f.words.is_none() {
        under.push(Span::styled(
            "keys.json can bind it",
            Tone::Dim.style(theme),
        ));
    }
    vec![
        pad(Vec::new()),
        BoxLine::spans(kit::fit(middle, inside, Style::new())),
        pad(under),
    ]
}

impl Overlay for FindKey {
    fn kind(&self) -> &'static str {
        "find-key"
    }

    fn stands_out(&self) -> Option<Rect> {
        self.drawn.as_ref().map(|d| d.outer)
    }

    fn draw(&mut self, buf: &mut Buffer, look: &Look) {
        let lines = self.lines(&look.theme);
        let mut spec = BoxSpec::new("Find a key", &lines);
        spec.width = INSIDE;
        self.drawn = Some(draw::draw_box(buf, &spec, look));
    }

    fn key(&mut self, key: Key, look: &Look) -> Step {
        // Enter and Esc alone work the box; M-Enter and the like are looked up.
        let alone = key.mods == Mods::NONE && !self.after_prefix;
        match key.code {
            KeyCode::Esc if alone => Step::Close(Outcome::Dismissed),
            KeyCode::Enter if alone => match self.focus {
                Some(Cap::Close) => Step::Close(Outcome::Dismissed),
                _ => self.show(),
            },
            // Tab and Shift-Tab alone are looked up and move the focus
            // through the keycaps too; M-Tab is only looked up.
            KeyCode::Tab | KeyCode::BackTab
                if look.tab_focus
                    && !self.after_prefix
                    && (key.mods == Mods::NONE || key.mods == Mods::SHIFT) =>
            {
                let focus = self.focus;
                self.look_up(key);
                let back = key.code == KeyCode::BackTab || key.mods == Mods::SHIFT;
                self.focus = match (focus, back) {
                    (None, false) | (Some(Cap::Close), true) => Some(Cap::Show),
                    (Some(Cap::Show), false) | (None, true) => Some(Cap::Close),
                    (Some(Cap::Close), false) | (Some(Cap::Show), true) => None,
                };
                Step::Redraw
            }
            _ => {
                self.focus = None;
                self.look_up(key);
                Step::Redraw
            }
        }
    }

    fn mouse(&mut self, mouse: Mouse, look: &Look) -> Step {
        let hit = self.drawn.as_ref().map(|d| d.hit(mouse.col, mouse.row));
        let cap = match hit {
            Some(Hit::Line(line, col)) if line == self.caps_line => self
                .cap_cols
                .iter()
                .find(|(_, from, to)| (*from..=*to).contains(&col))
                .map(|(c, _, _)| *c),
            _ => None,
        };
        match draw::gesture(&mouse) {
            Gesture::Click if look.mouse => match (hit, cap) {
                (Some(Hit::Outside | Hit::Close), _) => Step::Close(Outcome::Dismissed),
                (_, Some(Cap::Show)) => self.show(),
                (_, Some(Cap::Close)) => Step::Close(Outcome::Dismissed),
                _ => Step::Same,
            },
            _ if look.hover && cap != self.hover => {
                self.hover = cap;
                Step::Redraw
            }
            _ => Step::Same,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mission::keymap::{Shown, items};
    use crate::ui::draw::tests::screen;

    fn lookup() -> Lookup {
        let bindings = Bindings::default();
        let shown = Shown {
            github: true,
            linear: true,
            ask: true,
            easy: false,
        };
        let counts: HashMap<String, u64> = [("M-j".to_string(), 212)].into_iter().collect();
        Lookup {
            rows: items(&bindings, shown, &counts),
            bindings,
            counts: Some(counts),
        }
    }

    fn key(n: &str) -> Key {
        Key::parse(n).unwrap()
    }

    fn text(f: &mut FindKey) -> String {
        let look = Look::default();
        let mut buf = Buffer::empty(Rect::new(0, 0, 100, 30));
        f.draw(&mut buf, &look);
        screen(&buf).join("\n")
    }

    #[test]
    fn a_key_pressed_comes_up_with_what_it_does_and_the_one_before_under_it() {
        let mut f = FindKey::new(lookup(), false);
        let look = Look::default();
        assert!(text(&mut f).contains(PROMPT));
        assert_eq!(f.key(key("M-j"), &look), Step::Redraw);
        let shown = text(&mut f);
        assert!(shown.contains("M-j"), "{shown}");
        assert!(shown.contains("Search and jump to a tab"), "{shown}");
        assert!(shown.contains("#tabs #navigate #find"), "{shown}");
        assert!(shown.contains("default"), "{shown}");
        assert!(shown.contains("212 this week"), "{shown}");
        assert!(!shown.contains("Before it"), "{shown}");
        f.key(key("M-u"), &look);
        f.key(key("M-j"), &look);
        let shown = text(&mut f);
        assert!(
            shown.contains("Before it, M-u: nothing bound; keys.json can bind it"),
            "{shown}"
        );
    }

    #[test]
    fn enter_shows_the_key_at_its_row_and_an_apps_key_is_named_not_pressed() {
        let look = Look::default();
        let mut f = FindKey::new(lookup(), false);
        f.key(key("M-j"), &look);
        let row = f.found().unwrap().row.unwrap();
        assert_eq!(keymap::press_of(&f.lookup.rows[row]), Some(Action::FindTab));
        assert_eq!(
            f.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Close(Outcome::Chose(row))
        );
        let mut f = FindKey::new(lookup(), false);
        assert_eq!(f.key(key("j"), &look), Step::Redraw);
        let found = f.found().unwrap();
        assert_eq!(found.app.as_deref(), Some("Sidebar"));
        assert!(text(&mut f).contains("Sidebar: "));
        // An app's label spelled its own way: ^d is C-d; a lone Space
        // isn't Notes' leader sequence.
        let mut f = FindKey::new(lookup(), false);
        f.key(Key::new(KeyCode::Char('d'), Mods::CTRL), &look);
        assert_eq!(f.found().unwrap().app.as_deref(), Some("Sidebar"));
        assert_eq!(label_keys("Space b"), Vec::<String>::new());
        assert_eq!(label_keys("h esc q ^c"), ["h", "Escape", "q", "C-c"]);
        assert_eq!(label_keys("Ctrl-F"), ["C-f"]);
        assert_eq!(label_keys("1-3"), ["1", "2", "3"]);
        assert_eq!(label_keys("↵ on a heading"), ["Enter"]);
        assert_eq!(label_keys("tab ⇧tab"), ["Tab", "BTab"]);
        assert_eq!(label_keys("j k"), ["j", "k", "Down", "Up"]);
        let mut f = FindKey::new(lookup(), false);
        f.key(Key::new(KeyCode::Down, Mods::NONE), &look);
        assert_eq!(f.found().unwrap().app.as_deref(), Some("Sidebar"));
        let mut f = FindKey::new(lookup(), false);
        f.key(key("a"), &look);
        assert!(
            !f.found().unwrap().detail.contains("heading"),
            "{:?}",
            f.found()
        );
        // A digit the sidebar and the Notifications view both take names both.
        let mut f = FindKey::new(lookup(), false);
        f.key(key("3"), &look);
        let found = f.found().unwrap();
        assert_eq!(found.app.as_deref(), Some("Sidebar"));
        assert!(found.detail.contains("Mission control: "), "{found:?}");
        assert_eq!(
            label_keys("⇧↵ ^j"),
            [
                Key::new(KeyCode::Enter, Mods::SHIFT).notation(),
                "C-j".to_string()
            ]
        );
        assert_eq!(
            label_keys("Ctrl-Shift-P"),
            [Key::new(KeyCode::Char('p'), Mods::CTRL.union(Mods::SHIFT)).notation()]
        );
        let mut f = FindKey::new(lookup(), false);
        f.key(Key::new(KeyCode::Char('f'), Mods::CTRL), &look);
        assert_eq!(f.found().unwrap().words.as_deref(), Some("find a note"));
        // Easy mode's ⌘S, as Ghostty sends it.
        let mut easy = lookup();
        let shown = Shown {
            github: true,
            linear: true,
            ask: true,
            easy: true,
        };
        easy.rows = items(&easy.bindings, shown, &HashMap::new());
        let cmd_s = Key::new(
            KeyCode::Char('s'),
            Mods::CTRL.union(Mods::ALT).union(Mods::SHIFT),
        );
        let found = easy.find(cmd_s, false);
        assert!(
            found
                .words
                .as_deref()
                .is_some_and(|w| w.starts_with("save")),
            "{found:?}"
        );
        // A prefix key the palette lists under its root key's row still
        // has that row: M-b x under M-w's.
        let mut f = FindKey::new(lookup(), false);
        f.key(key("M-b"), &look);
        f.key(key("x"), &look);
        let row = f.found().unwrap().row.unwrap();
        assert_eq!(
            keymap::action_of(&f.lookup.rows[row]),
            Some(Action::ClosePane)
        );
        // M-4 shows M-1's shared row; M-Tab is looked up, not a focus move.
        let mut f = FindKey::new(lookup(), false);
        f.key(key("M-4"), &look);
        let row = f.found().unwrap().row.unwrap();
        assert_eq!(
            keymap::action_of(&f.lookup.rows[row]),
            Some(Action::SelectTab(1))
        );
        // Tab is looked up and moves the focus to the first keycap.
        f.key(Key::new(KeyCode::Tab, Mods::NONE), &look);
        assert_eq!(f.found().unwrap().key, "Tab");
        assert_eq!(f.focus, Some(Cap::Show));
        f.key(Key::new(KeyCode::BackTab, Mods::NONE), &look);
        assert_eq!(f.focus, None, "Shift-Tab goes back");
        // M-p is the palette's, Notes in front too.
        f.key(key("M-p"), &look);
        assert!(
            !f.found().unwrap().detail.contains("Notes: "),
            "{:?}",
            f.found()
        );
        f.key(Key::new(KeyCode::Enter, Mods::ALT), &look);
        assert_eq!(f.found().unwrap().key, "M-Enter");
        f.key(key("M-b"), &look);
        f.key(key("0"), &look);
        assert!(
            f.found().unwrap().row.is_some(),
            "M-b 0 has a row: {:?}",
            f.found()
        );
        f.key(key("M-Tab"), &look);
        assert_eq!(
            f.found().unwrap().words.as_deref(),
            Some("Go back to the previous tab")
        );
        // After the prefix, a key is looked up as one after it.
        let mut f = FindKey::new(lookup(), false);
        f.key(key("M-b"), &look);
        f.key(key("d"), &look);
        assert_eq!(f.found().unwrap().key, "M-b d");
        assert_eq!(f.found().unwrap().count, None, "the habits don't count it");
        assert!(f.found().unwrap().words.is_some());
        // Enter after the prefix is looked up, not the box's Enter.
        f.key(key("M-b"), &look);
        assert_eq!(
            f.key(Key::new(KeyCode::Enter, Mods::NONE), &look),
            Step::Redraw
        );
        assert_eq!(f.found().unwrap().key, "M-b Enter");
        assert_eq!(
            f.key(Key::new(KeyCode::Esc, Mods::NONE), &look),
            Step::Close(Outcome::Dismissed)
        );
    }
}
