//! The app driven as someone would drive it, ported from test/notes.test.js:
//! each test gets a notes directory of its own, keys are typed and the
//! mouse clicked through the app's own input, and the screen is drawn into
//! a buffer and read back. Nothing here reaches your notes, the clipboard,
//! a terminal or the workspace's server.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::mux::input::{Mods, MouseButton, MouseKind};
use crate::notes::store::tests::{Dir, set_mtime, temp, write};

fn options() -> Options {
    Options {
        images: false,
        ..Options::default()
    }
}

fn app_in(store: Store, opts: Options) -> (App, Receiver<Msg>) {
    let (tx, rx) = channel();
    let mut app = App::new(store, opts, 120, 30, tx);
    app.claude = None;
    (app, rx)
}

fn started(name: &str) -> (Dir, App) {
    let (dir, store) = temp(name);
    let (mut app, _) = app_in(store, options());
    app.start();
    (dir, app)
}

/// Keys as Vim writes them: characters, and <Esc>, <CR>, <C-p> and the like.
fn keys(app: &mut App, keys: &str) {
    let mut rest = keys;
    while !rest.is_empty() {
        let key = if rest.starts_with('<')
            && let Some(end) = rest.find('>')
        {
            let name = &rest[1..end];
            rest = &rest[end + 1..];
            match name {
                "Esc" => Key::new(KeyCode::Esc, Mods::NONE),
                "CR" => Key::new(KeyCode::Enter, Mods::NONE),
                "Tab" => Key::new(KeyCode::Tab, Mods::NONE),
                "BS" => Key::new(KeyCode::Backspace, Mods::NONE),
                "Space" => Key::new(KeyCode::Char(' '), Mods::NONE),
                "C-S-p" => Key::new(
                    KeyCode::Char('p'),
                    Mods {
                        ctrl: true,
                        shift: true,
                        alt: false,
                    },
                ),
                "D-s" => Key::new(
                    KeyCode::Char('s'),
                    Mods {
                        ctrl: true,
                        shift: true,
                        alt: true,
                    },
                ),
                "D-z" => Key::new(
                    KeyCode::Char('z'),
                    Mods {
                        ctrl: true,
                        shift: true,
                        alt: true,
                    },
                ),
                n if n.starts_with("C-") => {
                    Key::new(KeyCode::Char(n.chars().nth(2).unwrap()), Mods::CTRL)
                }
                other => panic!("no key {other}"),
            }
        } else {
            let c = rest.chars().next().unwrap();
            rest = &rest[c.len_utf8()..];
            Key::new(KeyCode::Char(c), Mods::NONE)
        };
        app.key(key);
    }
}

fn screen(app: &mut App) -> Vec<String> {
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut buf);
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect()
}

fn lines(app: &App) -> Vec<String> {
    app.editor.doc.text.lines()
}

fn name(app: &App) -> String {
    app.current_path()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn click(app: &mut App, x: u16, y: u16) {
    for kind in [MouseKind::Press, MouseKind::Release] {
        app.mouse(Mouse {
            kind,
            button: MouseButton::Left,
            col: x,
            row: y,
            mods: Mods::NONE,
        });
    }
}

/// Where a text shows on the screen.
fn find(shown: &[String], text: &str) -> (u16, u16) {
    for (y, line) in shown.iter().enumerate() {
        if let Some(at) = line.find(text) {
            return (line[..at].chars().count() as u16, y as u16);
        }
    }
    panic!("{text:?} isn't on the screen:\n{}", shown.join("\n"));
}

#[test]
fn notes_opens_the_list_and_the_newest_note_a_welcome_note_at_first() {
    let (dir, mut app) = started("start");
    assert!(app.list_shown);
    assert_eq!(name(&app), "Welcome to Notes.md");
    assert!(
        app.list_rows
            .iter()
            .any(|r| r.text.trim_start().starts_with("All Notes") && r.text.ends_with(" 1")),
        "the list counts the note"
    );
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("☐ Click a box")),
        "drawn as Notes shows it:\n{}",
        shown.join("\n")
    );
    assert!(shown.iter().any(|l| l.contains("▾ Welcome to Notes")));
    app.new_note();
    assert_eq!(
        (app.editor.doc.row, app.editor.doc.col),
        (0, 2),
        "a new note starts with its title to type, after the #"
    );
    assert_eq!(app.editor.mode, Mode::Insert);
    keys(&mut app, "Groceries<CR><CR>- [ ] eggs #food<Esc>");
    app.save();
    assert_eq!(
        name(&app),
        "Groceries.md",
        "a new note is named once it has a title"
    );
    assert!(!dir.path.join("New Note.md").exists());
    // What a click does: it ticks only on the box itself.
    assert!(!app.act(2, Some(9), true));
    assert!(app.act(2, Some(0), true));
    assert_eq!(app.editor.doc.line(2), "- [x] eggs #food");
    assert!(
        app.act(0, Some(0), false),
        "Enter on the title folds its section"
    );
    let kinds = render::kinds(&lines(&app));
    assert!(app.editor.folds.closed_at(&kinds, 0).is_some());
    app.show(None, Some("food".into()), None);
    assert_eq!(
        app.list_rows
            .iter()
            .filter(|r| r.text.contains("Groceries")
                && matches!(r.look, sidebar::Look::Title | sidebar::Look::Open))
            .count(),
        1,
        "a tag shows the notes that have it"
    );
    let mut files: Vec<String> = fs::read_dir(&dir.path)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".md"))
        .collect();
    files.sort();
    assert_eq!(files, ["Groceries.md", "Welcome to Notes.md"]);
}

#[test]
fn the_list_is_its_full_width_where_the_note_has_room_and_never_under_its_minimum() {
    assert_eq!(list_width(200), sidebar::WIDTH);
    assert_eq!(
        list_width(70),
        sidebar::MIN_WIDTH,
        "squeezed, it keeps its minimum"
    );
    assert_eq!(list_width(30), sidebar::MIN_WIDTH);
}

#[test]
fn notes_opens_the_note_open_last_through_renames_and_the_newest_once_it_is_deleted() {
    let (dir, mut store) = temp("last");
    let old = store.create("", Some("Old"));
    set_mtime(&old, util::now_secs() - 600);
    store.create("", Some("Newer"));
    store.set_pinned("Newer.md", true);
    let (mut app, _) = app_in(store.clone(), options());
    app.start();
    assert_eq!(
        name(&app),
        "Newer.md",
        "at first, the top of the list: the pinned note"
    );
    app.open_note(&old, false, false);
    keys(&mut app, "ggA, renamed<Esc>");
    app.save();
    assert_eq!(
        app.store.meta()["last"],
        "Old, renamed.md",
        "the one open last, under its new name"
    );
    app.stop();
    let (mut again, _) = app_in(store.clone(), options());
    again.start();
    assert_eq!(
        name(&again),
        "Old, renamed.md",
        "started again, it opens where you left off"
    );
    again.store.delete(&dir.path.join("Old, renamed.md"));
    assert!(
        again.store.meta().get("last").is_none(),
        "deleted, it is no longer the one to open"
    );
    assert!(again.store.last().is_none());
}

#[test]
fn an_item_ticked_from_outside_moves_under_the_ones_left_to_do_and_is_saved_that_way() {
    let (_dir, mut store) = temp("ticked");
    let path = store.create("", Some("Errands"));
    write(&path, "# Errands\n\n- [ ] milk\n- [ ] eggs\n- [ ] bread\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    write(&path, "# Errands\n\n- [x] milk\n- [ ] eggs\n- [ ] bread\n");
    app.handle(Msg::Watched);
    std::thread::sleep(WATCH_WAIT + Duration::from_millis(20));
    app.tick();
    assert!(
        lines(&app).join("\n").contains("bread\n\n- [x] milk"),
        "ticked from outside, it moved down in the open note"
    );
    app.tick();
    app.save();
    assert!(
        fs::read_to_string(&path)
            .unwrap()
            .contains("bread\n\n- [x] milk"),
        "and the file says so"
    );
}

#[test]
fn a_note_changed_outside_while_open_shows_the_change_and_a_save_keeps_both() {
    let (dir, mut store) = temp("outside");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n- [ ] two\n- [ ] three\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let given = crate::notes::id::of_file(&path).expect("given an ID as it opened");
    app.editor.doc.row = 4;
    app.editor.doc.col = 4;
    // Nothing unsaved: the change shows up on its own, the cursor where it was.
    write(
        &path,
        "# Plan\n\n- [ ] one (ABC-1)\n- [ ] two\n- [ ] three\n",
    );
    app.check();
    assert_eq!(
        lines(&app).join("\n"),
        "# Plan\n\n- [ ] one (ABC-1)\n- [ ] two\n- [ ] three"
    );
    assert!(!app.modified(), "and has nothing unsaved because of it");
    assert_eq!((app.editor.doc.row, app.editor.doc.col), (4, 4));
    // Typing not yet saved, and a change outside on the next line: the save
    // has both.
    app.editor
        .change(|doc| doc.text.set_line(3, "- [ ] two, typed"));
    write(
        &path,
        "# Plan\n\n- [ ] one (ABC-1)\n- [ ] two\n- [ ] three (ABC-2)\n",
    );
    app.save();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        format!(
            "# Plan\n\n- [ ] one (ABC-1)\n- [ ] two, typed\n- [ ] three (ABC-2)\n\n{}\n",
            crate::notes::id::line(&given)
        ),
        "with its ID, which the outside change left out, put back last"
    );
    // The app's own save doesn't come back as a change.
    let before = app.editor.doc.text.clone();
    app.check();
    assert_eq!(app.editor.doc.text, before);
    // Deleted outside: what is typed isn't written back under the old name.
    fs::remove_file(&path).unwrap();
    app.editor
        .change(|doc| doc.text.set_line(0, "# Plan, still"));
    app.save();
    assert!(!path.exists());
    assert!(!dir.path.join("Plan, still.md").exists());
    assert!(
        lines(&app)[0].contains("Plan, still"),
        "the text stays in the note window"
    );
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("moved or deleted outside Notes"))
    );
}

#[test]
fn a_note_keeps_its_title_line_and_its_name_whatever_empties_it() {
    let (dir, mut store) = temp("title-kept");
    let path = store.create("", Some("Errands"));
    write(&path, "# Errands\n\n- [ ] milk\n- [ ] eggs\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let body = |path: &Path| crate::notes::id::body_of(&fs::read_to_string(path).unwrap());
    // Emptied in the editor: the save puts it back.
    app.editor.change(|doc| doc.text.set_line(0, ""));
    app.save();
    assert_eq!(body(&path), "# Errands\n\n- [ ] milk\n- [ ] eggs\n");
    assert_eq!(lines(&app)[0], "# Errands");
    // Emptied by a write from outside, as the handoff left it: brought in
    // with the title, written back, and the file keeps its name.
    write(&path, "\n\n- [ ] milk\n- [ ] eggs\n");
    app.check();
    app.save();
    assert_eq!(body(&path), "# Errands\n\n- [ ] milk\n- [ ] eggs\n");
    assert!(!dir.path.join("- [ ] milk.md").exists());
    assert_eq!(name(&app), "Errands.md");
    // Emptied in Insert mode and saved there: the file keeps it, and the
    // editor gets it back once Insert mode ends.
    app.editor.change(|doc| doc.text.set_line(0, ""));
    app.editor.doc.row = 0;
    app.editor.mode = Mode::Insert;
    app.save();
    assert_eq!(lines(&app)[0], "", "left alone while it is typed over");
    assert!(
        app.modified(),
        "and unsaved, so a change from outside merges around it"
    );
    assert_eq!(body(&path), "# Errands\n\n- [ ] milk\n- [ ] eggs\n");
    assert_eq!(name(&app), "Errands.md");
    app.editor.mode = Mode::Normal;
    app.save();
    assert_eq!(lines(&app)[0], "# Errands");
    // Emptied from outside while the cursor waits in Insert mode on it.
    app.editor.mode = Mode::Insert;
    write(&path, "\n\n- [ ] milk\n- [ ] eggs\n");
    app.check();
    app.editor.mode = Mode::Normal;
    app.save();
    assert_eq!(body(&path), "# Errands\n\n- [ ] milk\n- [ ] eggs\n");
    assert_eq!(name(&app), "Errands.md");
    // Typed over in the editor: the new title is kept, and names the file.
    app.editor.change(|doc| doc.text.set_line(0, "# Groceries"));
    app.save();
    assert_eq!(name(&app), "Groceries.md");
    // Renamed to a heading its body has: the rename asked for is kept.
    app.editor.change(|doc| doc.text.set_line(3, "# Shopping"));
    app.save();
    let renamed = app.retitle(&dir.path.join("Groceries.md"), "Shopping");
    assert_eq!(app.store.relative(&renamed), "Shopping.md");
    assert_eq!(lines(&app)[0], "# Shopping");
    // Everything deleted: the title comes back, and undo still brings back
    // the rest, the title's return no step of its own.
    let whole = lines(&app);
    app.editor.change(|doc| doc.text = Text::from_file(""));
    app.save();
    assert_eq!(lines(&app)[0], "# Shopping");
    app.editor.undo();
    assert_eq!(lines(&app), whole);
    // Deleted with the cursor below: the cursor stays on its line.
    app.editor.change(|doc| {
        let rest: Vec<String> = doc.text.lines().into_iter().skip(1).collect();
        doc.text = Text::from_lines(&rest);
        doc.row = 2;
    });
    let on = lines(&app)[2].clone();
    app.save();
    assert_eq!(lines(&app)[0], "# Shopping");
    assert_eq!(lines(&app)[app.editor.doc.row], on);
}

#[test]
fn a_note_opened_from_outside_puts_the_lists_cursor_on_it() {
    let (_dir, mut store) = temp("reveal");
    store.create("", Some("Loose"));
    store.create_folder("Work");
    let work = store.create("Work", Some("Plan"));
    let (mut app, _) = app_in(store, options());
    app.start();
    app.show(Some(String::new()), None, None);
    let before = app.list_at;
    assert_eq!(
        app.request(socket::Request::Open("Work/Plan.md".into())),
        "ok"
    );
    assert_ne!(app.list_at, before);
    assert_eq!(
        app.list_rows[app.list_at].item,
        Some(Item::Note(work.clone())),
        "the cursor on it"
    );
    assert_eq!(
        app.list.folder, None,
        "its folder's view left for All Notes, which lists it"
    );
    assert_eq!(app.request(socket::Request::Shown), "ok Work/Plan.md");
    assert!(
        app.request(socket::Request::Open("../x.md".into()))
            .starts_with("error"),
        "never outside the notes"
    );
    // The open note deleted from the list: the cursor on the note opened
    // in its place.
    let loose = app.store.absolute("Loose.md");
    app.open_note(&loose, true, false);
    app.focus = Focus::List;
    app.delete();
    assert_ne!(app.current_path(), Some(loose.as_path()));
    let shown = app.current_path().unwrap().to_path_buf();
    assert_eq!(app.list_rows[app.list_at].item, Some(Item::Note(shown)));
}

#[test]
fn the_list_is_put_away_and_shown_again_and_notes_sidebar_off_starts_without_it() {
    let (_dir, mut store) = temp("list");
    let first = store.create("", Some("First"));
    write(&first, "# First\n#todo\n");
    let (mut app, _) = app_in(store.clone(), options());
    app.start();
    assert!(app.list_shown);
    app.request(socket::Request::Sidebar(false));
    assert!(!app.list_shown);
    assert_eq!(app.focus, Focus::Note);
    app.request(socket::Request::Sidebar(true));
    assert!(app.list_shown);
    // Ctrl-B, from a note, typing in it or not, and from the list.
    app.show_list(false);
    keys(&mut app, "<C-b>");
    assert!(app.list_shown);
    app.focus = Focus::List;
    keys(&mut app, "<C-b>");
    assert!(!app.list_shown && app.focus == Focus::Note);
    keys(&mut app, "i<C-b>");
    assert!(app.list_shown, "typing too");
    keys(&mut app, "<Esc>");
    // A search that leaves out the open note is still there when it comes back.
    app.show(None, None, Some("nothing like it".into()));
    keys(&mut app, "<C-b><C-b>");
    assert!(app.list_shown && app.list.query.as_deref() == Some("nothing like it"));
    app.show(None, None, None);
    app.show_list(false);
    assert!(app.follow(1, 1));
    assert!(
        app.list_shown && app.list.tag.as_deref() == Some("todo"),
        "a #tag followed brings it back with the tag's notes"
    );
    let (mut beside, _) = app_in(
        store,
        Options {
            list: false,
            ..options()
        },
    );
    beside.store.unlock();
    beside.start();
    assert!(!beside.list_shown, "started beside a tab, without it");
}

#[test]
fn ctrl_w_and_the_m_arrows_move_between_the_list_and_the_note() {
    let (_dir, mut app) = started("panes");
    app.focus = Focus::List;
    keys(&mut app, "<C-w>l");
    assert_eq!(app.focus, Focus::Note);
    keys(&mut app, "<C-w>h");
    assert_eq!(app.focus, Focus::List);
    let alt = |code| Key::new(code, Mods::ALT);
    app.key(alt(KeyCode::Right));
    assert_eq!(app.focus, Focus::Note);
    // Typing too, as vim-tmux-navigator's mapping was.
    keys(&mut app, "i");
    app.key(alt(KeyCode::Left));
    assert_eq!(app.focus, Focus::List);
    // Past the list's edge the workspace's pane is asked for (not here,
    // with no server), and the focus stays.
    app.key(alt(KeyCode::Left));
    assert_eq!(app.focus, Focus::List);
    app.opts.pane_arrows = false;
    app.key(alt(KeyCode::Right));
    assert_eq!(app.focus, Focus::List, "off, they do nothing");
}

#[test]
fn the_wheel_scrolls_a_note_and_the_list_past_where_their_cursors_were() {
    let (_dir, mut store) = temp("wheel");
    let long = store.create("", Some("Long"));
    let body: String = (1..=120).map(|i| format!("line {i}\n")).collect();
    write(&long, &format!("# Long\n{body}"));
    for i in 0..40 {
        let note = store.create("", Some(&format!("Short {i}")));
        write(&note, &format!("# Short {i}\n"));
    }
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&long, false, false);
    screen(&mut app);
    let wheel = |app: &mut App, in_list: bool, down: bool| {
        let r = if in_list { app.geo.list } else { app.geo.note };
        app.mouse(Mouse {
            kind: MouseKind::Press,
            button: if down {
                MouseButton::WheelDown
            } else {
                MouseButton::WheelUp
            },
            col: r.x + 2,
            row: r.y + 2,
            mods: Mods::NONE,
        })
    };
    // Down and down again, never held back by the cursor, which keeps up.
    let mut tops = Vec::new();
    for _ in 0..12 {
        wheel(&mut app, false, true);
        screen(&mut app);
        tops.push(app.editor.top);
        let row = app.editor.doc.row;
        assert!(row >= app.editor.top, "the cursor in the view: {row}");
    }
    assert!(tops.windows(2).all(|t| t[1] > t[0]), "{tops:?}");
    wheel(&mut app, false, false);
    screen(&mut app);
    assert!(app.editor.top < *tops.last().unwrap(), "and back up");
    // The list scrolls past its cursor, which stays where it was.
    app.focus = Focus::List;
    app.list_at = 0;
    screen(&mut app);
    for _ in 0..3 {
        wheel(&mut app, true, true);
    }
    screen(&mut app);
    assert_eq!((app.list_at, app.list_top), (0, 9));
    keys(&mut app, "j");
    screen(&mut app);
    assert_eq!(app.list_top, 1, "a key brings the cursor back in sight");
}

/// A narrow app, as beside a tab, on a note of `count` lines after its
/// title, each `L<n>` and words enough to wrap to several rows.
fn tall_lines(name: &str, rows: u16, count: usize, words: usize) -> (Dir, App) {
    let (dir, mut store) = temp(name);
    let note = store.create("", Some("Tall"));
    let body: String = (1..=count)
        .map(|i| format!("L{i:02} {}\n", "word ".repeat(words).trim_end()))
        .collect();
    write(&note, &format!("# Tall\n{body}"));
    let (tx, _) = channel();
    let mut app = App::new(
        store,
        Options {
            list: false,
            ..options()
        },
        40,
        rows,
        tx,
    );
    app.claude = None;
    app.start();
    app.open_note(&note, false, false);
    screen(&mut app);
    (dir, app)
}

/// The screen rows the note's line `L<n>` takes, from its first to past
/// its last, when all of it shows.
fn whole(app: &App, shown: &[String], n: usize) -> Option<(usize, usize)> {
    let text = app.geo.text;
    let (from, to) = (text.y as usize, text.bottom() as usize);
    let start = (from..to).find(|&y| shown[y].contains(&format!("L{n:02}")))?;
    let end = (start + 1..to)
        .find(|&y| !shown[y].trim_start().starts_with("word"))
        .unwrap_or(to);
    // A line cut off at the view's end is followed by nothing, so it is
    // whole only when the next line starts under it or the note ends.
    let cut = end == to
        && n < lines(app).len() - 1
        && !shown[..to]
            .iter()
            .any(|l| l.contains(&format!("L{:02}", n + 1)));
    let rows = whole_height(app);
    (!cut || end - start == rows).then_some((start, end))
}

/// How many rows each of `tall_lines`' lines wraps to.
fn whole_height(app: &App) -> usize {
    let line = &lines(app)[1];
    let mut rows = 1;
    let mut used = 0;
    for word in line.split(' ') {
        let w = word.len() + usize::from(used > 0);
        if used + w > app.geo.text.width as usize {
            rows += 1;
            used = word.len();
        } else {
            used += w;
        }
    }
    rows
}

#[test]
fn the_view_stays_still_as_the_cursor_moves_through_lines_that_wrap_tall() {
    let (_dir, mut app) = tall_lines("tall-still", 24, 30, 36);
    let view = app.editor.height;
    let tall = whole_height(&app);
    assert!(tall >= 5 && tall * 2 <= view, "{tall} rows in {view}");
    // Down the note: the cursor's line always shows whole, the view moves a
    // line at most (the title's one row going with the first), and drawing
    // again moves nothing.
    for _ in 0..12 {
        let before = app.editor.top;
        keys(&mut app, "j");
        let shown = screen(&mut app);
        let (row, top) = (app.editor.doc.row, app.editor.top);
        assert!(
            whole(&app, &shown, row).is_some(),
            "L{row} whole:\n{shown:#?}"
        );
        let most = if before == 0 { 2 } else { 1 };
        assert!((before..=before + most).contains(&top), "{before} to {top}");
        assert_eq!(screen(&mut app), shown, "the same drawn again");
        assert_eq!(app.editor.top, top);
    }
    // Along a line in the middle of the note, nothing moves: before, every
    // key flipped the view between the line at its top and at its bottom.
    let (top, shown) = (app.editor.top, screen(&mut app));
    for key in ["l", "l", "l", "l", "l", "w", "h", "$", "0"] {
        keys(&mut app, key);
        assert_eq!(screen(&mut app), shown, "after {key}");
        assert_eq!(app.editor.top, top, "after {key}");
    }
    // And back up the same way.
    for _ in 0..12 {
        let before = app.editor.top;
        keys(&mut app, "k");
        let shown = screen(&mut app);
        let (row, top) = (app.editor.doc.row, app.editor.top);
        let on_the_title = row == 0;
        assert!(
            on_the_title || whole(&app, &shown, row).is_some(),
            "L{row} whole:\n{shown:#?}"
        );
        let most = if top == 0 { 2 } else { 1 };
        assert!((top..=top + most).contains(&before), "{before} to {top}");
        assert_eq!(screen(&mut app), shown, "the same drawn again");
    }
    assert_eq!(app.editor.top, 0);
    // Where the view has room for more lines than the scrolloff keeps, the
    // cursor moves between them without the view moving.
    let (_dir, mut app) = tall_lines("tall-within", 44, 30, 36);
    keys(&mut app, "10j");
    screen(&mut app);
    let top = app.editor.top;
    for key in ["k", "k", "j", "j"] {
        keys(&mut app, key);
        screen(&mut app);
        assert_eq!(app.editor.top, top, "after {key}");
    }
}

#[test]
fn a_line_taller_than_the_view_sits_at_its_top_and_zz_centers_by_rows() {
    let (_dir, mut app) = tall_lines("tall-over", 16, 8, 120);
    let view = app.editor.height;
    assert!(whole_height(&app) > view);
    for _ in 0..4 {
        keys(&mut app, "j");
        let shown = screen(&mut app);
        let row = app.editor.doc.row;
        assert_eq!(app.editor.top, row);
        let at = app.geo.text.y as usize;
        assert!(shown[at].contains(&format!("L{row:02}")), "{shown:#?}");
        keys(&mut app, "ll");
        assert_eq!(screen(&mut app), shown, "still along it");
    }
    // zz puts the cursor's line in the middle by the rows the lines over
    // it take, not by as many lines as half the view has rows.
    let (_dir, mut app) = tall_lines("tall-zz", 30, 30, 18);
    let (view, tall) = (app.editor.height, whole_height(&app));
    assert!(tall >= 3 && tall * 4 <= view, "{tall} rows in {view}");
    keys(&mut app, "15jzz");
    let shown = screen(&mut app);
    let (start, end) = whole(&app, &shown, 15).expect("the cursor's line whole");
    let text = app.geo.text;
    let (above, below) = (start - text.y as usize, text.bottom() as usize - end);
    assert!(above.abs_diff(below) <= tall, "{above} over, {below} under");
    assert_eq!(screen(&mut app), shown, "the same drawn again");
}

#[test]
fn the_wheel_over_lines_that_wrap_tall_lands_the_cursor_on_a_line_shown_whole() {
    let (_dir, mut app) = tall_lines("tall-wheel", 24, 40, 36);
    let wheel = |app: &mut App, down: bool| {
        let r = app.geo.note;
        app.mouse(Mouse {
            kind: MouseKind::Press,
            button: if down {
                MouseButton::WheelDown
            } else {
                MouseButton::WheelUp
            },
            col: r.x + 2,
            row: r.y + 2,
            mods: Mods::NONE,
        })
    };
    let mut tops = vec![app.editor.top];
    for _ in 0..6 {
        wheel(&mut app, true);
        let shown = screen(&mut app);
        let row = app.editor.doc.row;
        assert!(
            whole(&app, &shown, row).is_some(),
            "L{row} whole:\n{shown:#?}"
        );
        tops.push(app.editor.top);
    }
    assert!(tops.windows(2).all(|t| t[1] == t[0] + 3), "{tops:?}");
    for _ in 0..3 {
        wheel(&mut app, false);
        let shown = screen(&mut app);
        let row = app.editor.doc.row;
        assert!(
            whole(&app, &shown, row).is_some(),
            "L{row} whole:\n{shown:#?}"
        );
        tops.push(app.editor.top);
    }
    assert_eq!(tops[tops.len() - 4..], [18, 15, 12, 9], "never held back");
}

#[test]
fn a_note_keeps_its_undo_history_and_folds_while_another_is_open() {
    let (_dir, mut store) = temp("undo");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\n\n## Part\nunder it\n\nbody\n");
    let second = store.create("", Some("Beta"));
    write(&second, "# Beta\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "Goadded<Esc>");
    keys(&mut app, "ggjjza");
    let folds = app.editor.folds.clone();
    assert_ne!(folds, Default::default(), "a section folded");
    app.open_note(&second, false, false);
    app.open_note(&first, false, false);
    assert_eq!(app.editor.folds, folds, "still folded");
    keys(&mut app, "u");
    assert!(
        !lines(&app).contains(&"added".to_string()),
        "u undoes what was typed before: {:?}",
        lines(&app)
    );
    // A change made to its file meanwhile is a step of its own to undo.
    app.open_note(&second, false, false);
    write(&first, "# Alpha\n\nfrom outside\n");
    app.open_note(&first, false, false);
    assert!(lines(&app).contains(&"from outside".to_string()));
    keys(&mut app, "u");
    assert!(
        lines(&app).contains(&"## Part".to_string()),
        "{:?}",
        lines(&app)
    );
}

#[test]
fn only_one_app_opens_the_notes_and_another_says_so() {
    let (dir, store) = temp("locked");
    // Held by a live process: the test runner's parent.
    // SAFETY: getppid has no preconditions.
    let parent = unsafe { libc::getppid() } as i64;
    fs::write(dir.path.join(".notes.lock"), parent.to_string()).unwrap();
    let (mut app, _) = app_in(store, options());
    app.start();
    let shown = screen(&mut app);
    assert!(shown.iter().any(|l| l.contains(&format!(
        "Notes is open already, in another window (process {parent})"
    ))));
    assert!(
        !fs::read_dir(&dir.path)
            .unwrap()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().ends_with(".md")),
        "and not even the welcome note written"
    );
    keys(&mut app, "q");
    assert!(app.quit);
    app.stop();
    assert_eq!(
        fs::read_to_string(dir.path.join(".notes.lock")).unwrap(),
        parent.to_string(),
        "the lock is left to its holder"
    );
}

#[test]
fn quitting_lets_the_lock_go() {
    let (dir, mut app) = started("quit");
    let me = std::process::id().to_string();
    assert_eq!(
        fs::read_to_string(dir.path.join(".notes.lock")).unwrap(),
        me
    );
    keys(&mut app, ":q<CR>");
    assert!(app.quit);
    app.stop();
    assert!(!dir.path.join(".notes.lock").exists(), "gone once it quits");
}

#[test]
fn deleting_the_open_note_from_the_list_moves_it_to_recently_deleted_and_shows_the_next() {
    let (_dir, mut store) = temp("delete");
    let keep = store.create("", Some("Keep"));
    set_mtime(&keep, util::now_secs() - 60);
    store.create("", Some("Drop"));
    let (mut app, _) = app_in(store, options());
    app.start();
    assert_eq!(name(&app), "Drop.md", "the newest note opens first");
    app.focus = Focus::List;
    keys(&mut app, "dd");
    assert_eq!(name(&app), "Keep.md");
    let trash: Vec<String> = app
        .store
        .notes(Some(store::TRASH))
        .into_iter()
        .map(|n| n.title)
        .collect();
    assert_eq!(trash, ["Drop"]);
}

#[test]
fn a_style_picked_from_the_format_menu_goes_on_to_typing_and_ticking_doesnt() {
    let (_dir, mut app) = started("menu");
    app.new_note();
    keys(&mut app, "<Esc>");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# Groceries", "", "eggs", "- [ ] milk"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "<C-p>");
    assert_eq!(app.top_kind(), Some("format-menu"));
    keys(&mut app, "8");
    assert_eq!(app.editor.doc.line(2), "* eggs");
    assert_eq!(
        app.editor.mode,
        Mode::Insert,
        "a bulleted list is picked to write in"
    );
    keys(&mut app, "<Esc>");
    app.editor.doc.row = 3;
    keys(&mut app, "<C-p>c");
    assert_eq!(app.editor.doc.line(3), "- [x] milk");
    assert_eq!(
        app.editor.mode,
        Mode::Normal,
        "ticking a box is done, not written in"
    );
    keys(&mut app, "u");
    assert_eq!(
        app.editor.doc.line(3),
        "- [ ] milk",
        "and undone as one change"
    );
}

#[test]
fn the_leader_keys_style_the_word_or_the_selection_and_dot_repeats() {
    let (_dir, mut app) = started("leader");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "hello big world", "one two"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 1;
    keys(&mut app, "<Space>b");
    assert_eq!(app.editor.doc.line(2), "**hello** big world");
    keys(&mut app, "j0vee<Space>i");
    assert_eq!(app.editor.doc.line(3), "*one two*", "on the selection");
    keys(&mut app, "kddu");
    assert_eq!(lines(&app).len(), 4, "dd undone");
    keys(&mut app, "ggjj0dw.");
    assert_eq!(
        app.editor.doc.line(2),
        "** big world",
        ". does it again: the markers, then the word"
    );
}

#[test]
fn the_toolbar_has_a_button_for_its_items_and_a_click_styles_the_word() {
    let (_dir, mut app) = started("toolbar");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# Title", "", "hello big world"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 1;
    let shown = screen(&mut app);
    let (x, y) = find(&shown, " B ");
    assert_eq!(y, 0, "over the note");
    click(&mut app, x + 1, y);
    assert_eq!(
        app.editor.doc.line(2),
        "**hello** big world",
        "B bolds the word under the cursor"
    );
    let (mut bare, _) = app_in(
        temp("bare").1,
        Options {
            toolbar: false,
            ..options()
        },
    );
    bare.start();
    assert!(
        !screen(&mut bare).iter().any(|l| l.contains(" H1 ")),
        "with the toolbar off, no toolbar"
    );
}

#[test]
fn a_click_ticks_a_box_and_folds_a_heading_and_moves_the_cursor_elsewhere() {
    let (_dir, mut store) = temp("click");
    let path = store.create("", Some("List"));
    write(
        &path,
        "# List\n\n- [ ] eggs\n- [ ] milk\n\n## Later\n\nsome text\n",
    );
    let (mut app, _) = app_in(
        store,
        Options {
            move_checked_to_bottom: false,
            ..options()
        },
    );
    app.start();
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "☐ eggs");
    click(&mut app, x, y);
    assert_eq!(app.editor.doc.line(2), "- [x] eggs");
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "milk");
    click(&mut app, x + 1, y);
    assert_eq!(
        app.editor.doc.line(3),
        "- [ ] milk",
        "a click on the text only moves the cursor"
    );
    assert_eq!((app.editor.doc.row, app.editor.doc.col), (3, 7));
    let (x, y) = find(&shown, "▾ Later");
    click(&mut app, x, y);
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("▸ Later  (2 lines)")),
        "folded:\n{}",
        shown.join("\n")
    );
    assert!(!shown.iter().any(|l| l.contains("some text")));
}

#[test]
fn easy_mode_adds_the_cmd_keys_and_the_key_help_line() {
    let (_dir, mut app) = {
        let (dir, store) = temp("easy");
        let (mut app, _) = app_in(
            store,
            Options {
                easy: true,
                ..options()
            },
        );
        app.start();
        (dir, app)
    };
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("Shift+arrows select")),
        "the keys named at the bottom"
    );
    keys(&mut app, "ggA more<D-z>");
    assert!(
        !app.editor.doc.line(0).contains("more"),
        "Cmd-Z undoes, typing or not"
    );
    keys(&mut app, "<Esc>");
    let saved = app.store.absolute("Welcome to Notes.md");
    app.editor.change(|doc| doc.text.set_line(2, "Changed."));
    keys(&mut app, "<D-s>");
    assert!(
        fs::read_to_string(&saved).unwrap().contains("Changed."),
        "Cmd-S saves"
    );
}

#[test]
fn the_palette_runs_the_command_picked_where_it_was_opened() {
    let (_dir, mut store) = temp("palette");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\nShip it\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.editor.doc.row = 1;
    keys(&mut app, "<C-S-p>");
    assert_eq!(app.top_kind(), Some("picker"));
    keys(&mut app, "Paragraph: Heading<CR>");
    assert_eq!(
        app.editor.doc.line(1),
        "## Ship it",
        "the Format menu's Heading, on the cursor's line"
    );
    keys(&mut app, "<Esc><C-S-p>Show or put away the list<CR>");
    assert!(!app.list_shown, "the list put away from a note");
    keys(&mut app, "<C-S-p>");
    let shown = screen(&mut app);
    assert!(
        shown
            .iter()
            .any(|l| l.contains("Show or put away the list") && l.contains("Ctrl-B")),
        "the one picked last first, with its key"
    );
    keys(&mut app, "<Esc>");
    let recent = palette::recent(&app.recent_file);
    assert_eq!(recent[0], "Show or put away the list");
}

#[test]
fn gw_on_an_item_saves_the_note_and_presses_work_on_item_and_elsewhere_says_why() {
    let (dir, mut store) = temp("gw");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] Fix the login\n- [x] Done\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    // An unsaved line above the item moves it, and the press names the line
    // on screen, saved.
    app.editor.doc.row = 1;
    keys(&mut app, "oA new first line<Esc>");
    app.editor.doc.row = 3;
    assert_eq!(
        app.item_press(),
        Some(vec![
            "work-on-item".to_string(),
            "Plan.md".to_string(),
            "4".to_string()
        ])
    );
    assert!(
        fs::read_to_string(dir.path.join("Plan.md"))
            .unwrap()
            .contains("A new first line\n- [ ] Fix the login"),
        "saved before the press"
    );
    app.editor.doc.row = 4;
    assert_eq!(app.item_press(), None);
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("that line is ticked"))
    );
    // gw itself, on a line that is no item, says so.
    app.editor.doc.row = 0;
    keys(&mut app, "gw");
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("isn't a checklist item"))
    );
}

#[test]
fn an_image_pasted_while_typing_goes_under_the_line_and_ctrl_v_else_types_the_next_key() {
    let (dir, mut store) = temp("ctrl-v");
    let path = store.create("", Some("Bug report"));
    write(&path, "# Bug report\n\n- [ ] the button is cut off\n");
    let (mut app, _) = app_in(store, options());
    app.grab = |file: &Path| fs::write(file, b"\x89PNG").is_ok();
    app.start();
    app.editor.doc.row = 2;
    keys(&mut app, "A<C-v><Esc>");
    let line = app.editor.doc.line(3);
    assert!(
        line.starts_with("  ![image-") && line.contains("](.attachments/Bug-report/image-"),
        "{line}"
    );
    app.grab = |_: &Path| false;
    app.editor.doc.row = 2;
    keys(&mut app, "A<C-v><Tab><Esc>");
    assert_eq!(
        app.editor.doc.line(2),
        "- [ ] the button is cut off\t",
        "with no image there, the next key as it is"
    );
    let pictures = dir.path.join(".attachments/Bug-report");
    assert_eq!(fs::read_dir(pictures).unwrap().count(), 1);
}

#[test]
fn a_note_renamed_from_the_list_takes_a_new_file_and_keeps_its_text() {
    let (_dir, mut store) = temp("rename");
    let plans = store.create("", Some("Plans"));
    fs::write(&plans, "# Plans\n\n- [ ] ship it\n").unwrap();
    let other = store.create("", Some("Other"));
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&other, false, false);
    let to = app.retitle(&plans, "Weekend plans");
    assert_eq!(
        app.store.relative(&to),
        "Weekend plans.md",
        "the file is named for the new title"
    );
    assert_eq!(
        fs::read_to_string(&to).unwrap(),
        "# Weekend plans\n\n- [ ] ship it\n",
        "only the title changed"
    );
    let numbered = app.retitle(&to, "Plans 2");
    let same = app.retitle(&numbered, "Plans: 2");
    assert_eq!(app.store.relative(&same), "Plans- 2.md");
    // The open note, renamed from the list.
    let open = app.current_path().unwrap().to_path_buf();
    let renamed = app.retitle(&open, "Renamed");
    assert_eq!(app.store.relative(&renamed), "Renamed.md");
    assert_eq!(
        app.current_path(),
        Some(renamed.as_path()),
        "the open note follows"
    );
}

#[test]
fn an_open_note_moved_to_another_folder_keeps_its_time() {
    let (_dir, mut store) = temp("move");
    store.create_folder("Work");
    let plan = store.create("", Some("Plan"));
    set_mtime(&plan, 1_767_225_600);
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&plan, true, false);
    app.reveal(&plan);
    app.focus = Focus::List;
    keys(&mut app, "m");
    assert_eq!(app.top_kind(), Some("menu"));
    keys(&mut app, "j<CR>");
    let to: PathBuf = app.store.absolute("Work/Plan.md");
    assert!(to.exists());
    assert_eq!(store::mtime(&to), Some(1_767_225_600));
    assert_eq!(app.current_path(), Some(to.as_path()));
}

#[test]
fn a_squash_asks_first_then_shows_its_progress_and_esc_cancels_it() {
    let (dir, mut store) = temp("squash");
    let a = store.create("", Some("First"));
    let b = store.create("", Some("Second"));
    // A Claude as strict as Claude Code about how it is called, which
    // takes a moment to answer.
    let claude = crate::stand_in::Claude::new()
        .answer("cat >/dev/null\nsleep 1\nprintf '# Both\\n'")
        .install(&dir.path.join(".bin"));
    let (mut app, rx) = app_in(store, options());
    app.start();
    app.claude = Some(claude);
    app.list.marked = vec![a.clone(), b.clone()];
    app.squash();
    let shown = screen(&mut app).join("\n");
    assert!(
        shown.contains("Squash these 2 notes into one? [Y/n]"),
        "{shown}"
    );
    assert!(shown.contains("· First") && shown.contains("· Second"));
    assert!(shown.contains("Claude (sonnet)") && shown.contains("Recently Deleted"));
    keys(&mut app, "n");
    assert!(app.top_kind().is_none(), "n closes it, squashing nothing");
    app.squash();
    keys(&mut app, "<CR>");
    let shown = screen(&mut app).join("\n");
    assert!(
        shown.contains("Squashing 2 notes with Claude (sonnet)"),
        "{shown}"
    );
    assert!(shown.contains("Esc cancels"));
    keys(&mut app, "<Esc>");
    assert!(app.top_kind().is_none(), "Esc closes it");
    if let Ok(Msg::Squashed(result)) = rx.recv_timeout(Duration::from_secs(10)) {
        app.handle(Msg::Squashed(result));
    }
    assert!(a.exists() && b.exists(), "the notes are untouched");
    assert!(!dir.path.join("Both.md").exists());
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("canceled"))
    );
}

#[test]
fn the_note_window_numbers_its_lines_relative_to_the_cursor() {
    let (_dir, mut app) = started("numbers");
    keys(&mut app, "gg");
    let shown = screen(&mut app);
    let x = app.geo.note.x as usize;
    let gutter = |y: usize| shown[y + 1].chars().skip(x).take(3).collect::<String>();
    assert_eq!(
        gutter(0).trim(),
        "1",
        "the cursor's line shows its own number"
    );
    assert_eq!(gutter(1).trim(), "1", "the next, how far it is");
    let (mut bare, _) = app_in(
        temp("bare-numbers").1,
        Options {
            numbers: false,
            ..options()
        },
    );
    bare.start();
    bare.draw(&mut Buffer::empty(Rect::new(0, 0, 120, 30)));
    assert_eq!(bare.geo.text.x, bare.geo.note.x, "off, the note is bare");
}

// test/notes.test.js's "each note has a stable ID as its last line: made
// with it, given one when first opened without it, kept last on save, put
// back when deleted, hidden, and a copy gets one of its own".
#[test]
fn each_note_keeps_a_stable_id_last_never_shown_given_once_and_a_copy_gets_its_own() {
    use crate::notes::id;
    let (dir, mut store) = temp("ids");
    let made = store.create("", Some("Fresh"));
    let made_text = fs::read_to_string(&made).unwrap();
    assert!(
        regex::Regex::new(r"^# Fresh\n\n\n<!-- note-id: n-[0-9a-f]{12} -->\n$")
            .unwrap()
            .is_match(&made_text),
        "a new note is made with its ID: {made_text:?}"
    );
    // An old note with no ID, last changed long ago.
    let old = dir.path.join("Old.md");
    write(&old, "# Old\n\n- [ ] one\n");
    set_mtime(&old, 1_767_225_600);
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&old, false, false);
    let given = fs::read_to_string(&old).unwrap();
    let note_id = id::in_text(&given).expect("given one as it opened");
    assert_eq!(
        given,
        format!("# Old\n\n- [ ] one\n\n{}\n", id::line(&note_id))
    );
    assert_eq!(
        crate::notes::store::mtime(&old),
        Some(1_767_225_600),
        "without its modified time changing, so it doesn't move up the list"
    );
    assert_eq!(lines(&app), ["# Old", "", "- [ ] one"], "and never shown");
    assert!(!screen(&mut app).iter().any(|l| l.contains("note-id")));
    // Typed at the end: the save keeps it last.
    keys(&mut app, "Go- [ ] two<Esc>");
    app.save();
    let below = format!("# Old\n\n- [ ] one\n- [ ] two\n\n{}\n", id::line(&note_id));
    assert_eq!(fs::read_to_string(&old).unwrap(), below);
    // Rewritten outside without it: the save puts it back.
    write(&old, "# Old\n\n- [ ] one\n- [ ] two\n");
    app.check();
    app.save();
    assert_eq!(fs::read_to_string(&old).unwrap(), below, "put back");
    // Another note's text pasted at its end, that note's ID line with it,
    // and one wrapped in bold.
    app.editor.change(|doc| {
        let end = doc.text.len();
        doc.text.set_lines(
            end,
            end,
            &[
                "# Other",
                "- [ ] pasted",
                "**<!-- note-id: n-bbbbbbbbbbbb -->**",
                "",
                "<!-- note-id: n-bbbbbbbbbbbb -->",
            ],
        );
    });
    app.save();
    assert_eq!(
        fs::read_to_string(&old).unwrap(),
        format!(
            "# Old\n\n- [ ] one\n- [ ] two\n# Other\n- [ ] pasted\n\n\n{}\n",
            id::line(&note_id)
        ),
        "another note's ID pasted in is dropped, and the note keeps its own"
    );
    assert!(!lines(&app).iter().any(|l| l.contains("note-id")));
    // A copy of the note's file, changed later, holds the same ID until
    // opened.
    let copy = dir.path.join("Old copy.md");
    fs::copy(&old, &copy).unwrap();
    set_mtime(&copy, crate::util::now_secs() + 10);
    app.open_note(&copy, false, false);
    let copied = id::of_file(&copy).unwrap();
    assert_ne!(copied, note_id, "a copy gets an ID of its own");
    assert_eq!(
        id::of_file(&old).as_deref(),
        Some(note_id.as_str()),
        "the original keeps its own"
    );
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("Old copy.md is a copy of Old.md")),
        "said once"
    );
    // Never a title, a preview or a search's find.
    assert_eq!(
        crate::notes::store::title_of("\n<!-- note-id: n-0123456789ab -->\n"),
        "New Note"
    );
    assert_eq!(
        crate::notes::store::preview_of("# T\n\nwords\n\n<!-- note-id: n-0123456789ab -->\n"),
        "words"
    );
    assert!(app.store.search("note").is_empty());
}

// test/notes.test.js's "renaming a new, empty note gives its first line
// the title, never its hidden ID line", and "a note's ID line is left out
// of the Format menu's paragraph styles ... and stays hidden inside an
// unclosed code block".
#[test]
fn renaming_a_new_empty_note_titles_its_first_line_and_a_pasted_id_is_never_styled_or_shown() {
    use crate::notes::id;
    let (_dir, mut store) = temp("id-title");
    let path = store.create("", None);
    let other = store.create("", Some("Other"));
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&other, false, false);
    let to = app.retitle(&path, "Named");
    let text = fs::read_to_string(&to).unwrap();
    assert!(text.starts_with("# Named\n"), "{text:?}");
    assert_eq!(
        text.matches("note-id").count(),
        1,
        "and the ID line kept, once"
    );
    assert!(id::in_text(&text).is_some());

    let line = "<!-- note-id: n-0123456789ab -->";
    let kinds = crate::notes::render::kinds(&["# T", "```", "code", line]);
    assert_eq!(
        kinds[3],
        crate::notes::render::Kind::Id,
        "hidden inside an unclosed code block"
    );
    let mut doc = crate::notes::format::Doc::new(Text::from_lines(&["# T", "one", line]));
    crate::notes::format::paragraph(&mut doc, 1, 2, crate::notes::format::Paragraph::Check);
    assert_eq!(doc.line(1), "- [ ] one");
    assert_eq!(doc.line(2), line, "the ID line isn't made a checklist item");
}

// UX 2.0: the background look's tabs matched to the items they are for.
#[test]
fn the_server_tabs_on_items_are_matched_by_title_autopilot_item_or_wtn_item() {
    let list = serde_json::json!({ "sessions": [{ "tabs": [
        { "id": "@13", "index": 3, "agent": "working", "options": { "@tab_title": "show uncommitted changes" },
          "panes": [{ "agent": { "state": "working", "phase": "review round 1" } }] },
        { "id": "@12", "index": 2, "agent": "waiting", "options": { "@autopilot_item": "stable IDs" },
          "panes": [{ "agent": { "state": "waiting", "wait": "git push" } }] },
        { "index": 4, "agent": "working", "options": {}, "path": "/repo/wt", "panes": [] },
        { "index": 7, "agent": "working", "options": {}, "path": "/repo/by-id", "panes": [] },
        { "index": 8, "agent": "working", "options": { "@tab_note": "Home.md" }, "path": "/repo/by-id", "panes": [] },
        { "index": 9, "agent": "working", "options": { "@tab_title": "Tidy up", "@tab_note": "Work.md" }, "path": "/repo/by-id", "panes": [] },
        { "index": 5, "agent": "done", "options": { "@tab_title": "finished" }, "panes": [] },
        { "index": 6, "agent": null, "options": { "@tab_title": "idle" }, "panes": [] },
    ]}]});
    let got = item_tabs_of(&list, |dir| match dir {
        "/repo/wt" => Some((None, "from git".to_string())),
        "/repo/by-id" => Some((
            Some("n-aaaaaaaaaaaa".to_string()),
            "Fix the login".to_string(),
        )),
        _ => None,
    });
    assert_eq!(
        got["show uncommitted changes"],
        render::ItemTab {
            id: "@13".into(),
            note: String::new(),
            note_id: String::new(),
            index: 3,
            waits: false,
            detail: "review round 1".into()
        }
    );
    assert_eq!(
        got["stable IDs"],
        render::ItemTab {
            id: "@12".into(),
            note: String::new(),
            note_id: String::new(),
            index: 2,
            waits: true,
            detail: "git push".into()
        }
    );
    assert_eq!(got["from git"].index, 4);
    let on_a = &got[&note_item_key("n-aaaaaaaaaaaa", "Fix the login")];
    assert_eq!((on_a.index, on_a.note_id.as_str()), (7, "n-aaaaaaaaaaaa"));
    assert!(
        !got.contains_key("Fix the login"),
        "a tab whose note is known isn't under the item alone"
    );
    assert!(
        !got.contains_key(&note_item_key("Home.md", "Fix the login")),
        "a note linked to the tab doesn't outweigh the note its claim names"
    );
    assert_eq!(
        got[&note_item_key("Work.md", "Tidy up")].index,
        9,
        "a claim for another item names no note of this one"
    );
    assert_eq!(got.len(), 5, "a finished or idle tab is no chip");
}

#[test]
fn the_format_menu_stands_out_over_the_note_with_a_shadow_of_half_blocks() {
    let (_dir, mut app) = started("standout");
    app.new_note();
    keys(&mut app, "<Esc>");
    let mut before = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut before);
    keys(&mut app, "<C-p>");
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut buf);
    let outer = app.layers.last().unwrap().overlay.stands_out().unwrap();
    let theme = app.look.theme;
    assert_eq!(
        buf[(outer.x + 1, outer.y + 1)].bg,
        crate::ui::kit::Surface::Sunken.color(&theme)
    );
    // Behind it, dimmed.
    assert_ne!(buf[(0, 0)].style(), before[(0, 0)].style());
    assert_eq!(buf[(outer.right(), outer.y + 1)].symbol(), "▌");
}

// UX 2.0's chips and a note's pictures, as the workspace reads the screen
// to draw the pictures as sixel (src/mux/sixel/place.rs): a picture's rows
// follow the chip row under its item, in order, from its first, as the
// note scrolls, and one cut by the bottom of the screen shows its first.
#[test]
fn a_picture_under_an_item_with_chips_scrolls_with_them_in_one_piece() {
    use crate::mux::PaneId;
    use crate::mux::sixel::place;
    let (dir, mut store) = temp("chips-pictures");
    // Pictures of 80 by 48 pixels: ten cells of 8 by 16 wide, three tall.
    for name in ["a.png", "b.png"] {
        image::RgbaImage::from_pixel(80, 48, image::Rgba([0, 0, 255, 255]))
            .save(dir.path.join(name))
            .unwrap();
    }
    let path = store.create("", Some("Plan"));
    // Lines enough that the second picture is cut by the bottom of the
    // note, over UX 2.0's "Add an item" row.
    let mut text = String::from("# Plan\n\n");
    for n in 0..15 {
        text.push_str(&format!("line {n}\n"));
    }
    text.push_str("- [ ] Pick a name, needs a choice ![name](a.png)\n");
    text.push_str("- [ ] Draw the logo, needs a choice\n  ![logo](b.png)\n");
    for n in 15..50 {
        text.push_str(&format!("line {n}\n"));
    }
    write(&path, &text);
    let opts = Options {
        images: true,
        ux2: true,
        list: false,
        ..options()
    };
    let (mut app, _) = app_in(store, opts);
    app.images.cell = (8, 16);
    app.start();
    // The screen's rows, and the pieces the workspace finds in it.
    let draw = |app: &mut App| {
        let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
        app.draw(&mut buf);
        let rows: Vec<String> = (0..buf.area.height)
            .map(|y| {
                (0..buf.area.width)
                    .map(|x| buf[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect();
        let drawn = place::Drawn::of(&buf, vec![(PaneId(1), buf.area)]);
        let bottom = buf.area.height;
        let pieces = place::find(&mut buf, &drawn, bottom);
        (rows, pieces)
    };
    // Each piece a picture from its first row, ten cells wide, under its
    // item's chip row ("needs a choice"), or under the line naming it
    // that is under the chips, as far up as the note shows (the toolbar
    // on the top row).
    let check = |rows: &[String], pieces: &[place::Piece]| {
        let screen = rows.join("\n");
        for p in pieces {
            assert_eq!((p.row, p.col, p.area.width), (0, 0, 10), "{p:?}\n{screen}");
            let y = p.area.y as usize;
            let above: &[&str] = if rows[y - 1].contains("needs a choice") {
                &["needs a choice", "Pick a name"]
            } else {
                &["logo", "needs a choice", "Draw the logo"]
            };
            for (n, want) in above.iter().enumerate() {
                let Some(at) = y.checked_sub(n + 1).filter(|at| *at >= 1) else {
                    break;
                };
                assert!(rows[at].contains(want), "{want} at {at}: {p:?}\n{screen}");
            }
        }
    };
    // From the top: the first picture whole, the second cut by the
    // bottom of the screen after its first two rows.
    let (rows, pieces) = draw(&mut app);
    check(&rows, &pieces);
    let heights: Vec<u16> = pieces.iter().map(|p| p.area.height).collect();
    assert_eq!(heights, vec![3, 2], "{pieces:?}\n{}", rows.join("\n"));
    assert_eq!(pieces[1].area.bottom(), app.geo.text.bottom());
    // The cursor down a line at a time, the note scrolling up under it: the
    // pictures and their chips go up together, whole, and each leaves
    // the screen with its line.
    let mut whole = 0;
    for row in 20..50 {
        app.editor.doc.row = row;
        let (rows, pieces) = draw(&mut app);
        check(&rows, &pieces);
        whole += pieces.iter().filter(|p| p.area.height == 3).count();
    }
    assert!(whole > 0, "the pictures seen whole as the note scrolled");
}

// UX 2.0's item keys and footer (board 7).
fn started_ux2(name: &str) -> (Dir, App) {
    let (dir, store) = temp(name);
    let (mut app, _) = app_in(
        store,
        Options {
            ux2: true,
            ..options()
        },
    );
    app.start();
    (dir, app)
}

#[test]
fn a_parked_section_opens_folded_to_its_count_and_the_footer_shows_the_item_keys() {
    let (_dir, mut app) = started_ux2("parked");
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] one\n\n## Defered\n- [ ] later one\n### Backlog\n- [ ] later two\n  why\n",
    );
    app.open_note(&path, false, false);
    let shown = screen(&mut app);
    assert!(
        shown
            .iter()
            .any(|l| l.contains("▸ Defered   2 items, parked")),
        "{shown:#?}"
    );
    assert!(!shown.iter().any(|l| l.contains("later one")));
    assert!(shown.iter().any(|l| l.contains("Add an item")));
    let footer = shown.last().unwrap();
    assert!(footer.contains("Enter its plan or tick"), "{footer}");
    assert!(footer.contains("M-p work this note"), "{footer}");
}

#[test]
fn ga_adds_an_item_to_the_end_of_the_top_list() {
    let (_dir, mut app) = started_ux2("add item");
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] one\n  After: Kit\n\n- [x] done\n## Later\n- [ ] parked\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "gaa new one");
    assert!(screen(&mut app).iter().any(|l| l.contains("a new one")));
    keys(&mut app, "<CR>");
    assert_eq!(app.adding, None);
    assert_eq!(
        lines(&app)[..6],
        [
            "# Design",
            "",
            "- [ ] one",
            "  After: Kit",
            "- [ ] a new one",
            ""
        ]
    );
    assert_eq!(app.editor.doc.row, 4);
    // Esc leaves it, adding nothing; u takes the item back.
    keys(&mut app, "gaskipped<Esc>u");
    assert!(
        !lines(&app)
            .iter()
            .any(|l| l.contains("skipped") || l.contains("new one"))
    );
}

#[test]
fn enter_on_an_item_opens_its_plan_and_ticks_one_with_none() {
    let (_dir, mut app) = started_ux2("plan");
    let plan = app.store.create("", Some("Plan - Kit"));
    write(&plan, "# Plan - Kit\n\nThe steps\n");
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] planned\n  Plan: [[Plan - Kit]]\n- [ ] bare\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "3G<CR>");
    assert_eq!(name(&app), "Plan - Kit.md");
    app.open_note(&path, false, false);
    keys(&mut app, "5G<CR>");
    assert_eq!(name(&app), "Design.md");
    assert!(
        lines(&app).iter().any(|l| l == "- [x] bare"),
        "{:?}",
        lines(&app)
    );
    // o with no tab on the item is Vim's.
    keys(&mut app, "3Go");
    assert_eq!(app.editor.mode, Mode::Insert);
}

#[test]
fn a_paste_while_adding_an_item_goes_into_the_input_not_the_note() {
    let (_dir, mut app) = started_ux2("paste item");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    app.open_note(&path, false, false);
    keys(&mut app, "ga");
    app.input(Input::Paste("pasted words\nsecond line".into()));
    assert_eq!(app.adding.as_deref(), Some("pasted words"));
    assert_eq!(lines(&app), ["# Design", "", "- [ ] one"]);
}

#[test]
fn o_goes_to_an_items_tab_only_when_the_tab_is_this_notes() {
    let (_dir, mut app) = started_ux2("item tab");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] Fix tests\n");
    app.open_note(&path, false, false);
    let tab = |note: &str| render::ItemTab {
        id: "@7".into(),
        note: note.into(),
        note_id: String::new(),
        index: 7,
        waits: false,
        detail: String::new(),
    };
    app.item_tabs = HashMap::from([(note_item_key("Other.md", "Fix tests"), tab("Other.md"))]);
    keys(&mut app, "3Go");
    assert_eq!(app.editor.mode, Mode::Insert, "another note's tab: Vim's o");
    keys(&mut app, "<Esc>u");
    app.item_tabs = HashMap::from([
        (note_item_key("Other.md", "Fix tests"), tab("Other.md")),
        (note_item_key("Design.md", "Fix tests"), tab("Design.md")),
    ]);
    keys(&mut app, "3Go");
    assert_eq!(
        app.editor.mode,
        Mode::Normal,
        "this note's tab: o goes there"
    );
    assert_eq!(lines(&app), ["# Design", "", "- [ ] Fix tests"]);
}

#[test]
fn a_tabs_chip_shows_only_on_the_note_its_item_is_in() {
    let (_dir, mut app) = started_ux2("chips by id");
    let work = app.store.create("", Some("Work"));
    write(
        &work,
        &id::with("# Work\n\n- [ ] Fix the login\n", "n-aaaaaaaaaaaa"),
    );
    let home = app.store.create("", Some("Home"));
    write(
        &home,
        &id::with("# Home\n\n- [ ] Fix the login\n", "n-bbbbbbbbbbbb"),
    );
    let list = serde_json::json!({ "sessions": [{ "tabs": [
        { "id": "@7", "index": 7, "agent": "working", "options": {}, "path": "/repo/wt", "panes": [] },
    ]}]});
    app.item_tabs = item_tabs_of(&list, |_| {
        Some((
            Some("n-aaaaaaaaaaaa".to_string()),
            "Fix the login".to_string(),
        ))
    });
    app.open_note(&work, false, false);
    assert_eq!(app.note_tabs()["Fix the login"].index, 7, "its own note");
    app.open_note(&home, false, false);
    assert!(
        app.note_tabs().is_empty(),
        "another note's item of the same words"
    );
}

#[test]
fn a_long_draft_in_add_an_item_shows_its_end() {
    let (_dir, mut app) = started_ux2("long item");
    app.cols = 80;
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    app.open_note(&path, false, false);
    keys(&mut app, "ga");
    keys(&mut app, &"word ".repeat(20));
    keys(&mut app, "THE END");
    let shown = screen(&mut app);
    let input = shown.iter().find(|l| l.contains("adds it")).unwrap();
    assert!(input.contains("…") && input.contains("THE END▏"), "{input}");
}

#[test]
fn m_enter_outside_the_workspace_keeps_the_draft() {
    let (_dir, mut app) = started_ux2("plan draft");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    app.open_note(&path, false, false);
    keys(&mut app, "gakept");
    app.key(Key::new(KeyCode::Enter, Mods::ALT));
    assert_eq!(app.adding.as_deref(), Some("kept"));
    assert_eq!(lines(&app), ["# Design", "", "- [ ] one"]);
}

#[test]
fn an_item_handed_to_plan_item_comes_back_until_it_reaches_the_note() {
    let (_dir, mut app) = started_ux2("plan handed");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    app.open_note(&path, false, false);
    // M-Enter handed it on, and the repository's picker was closed: ga
    // brings it back, even after a change elsewhere that names its words.
    let other = app.store.create("", Some("Other"));
    let handed = || {
        vec![Planning {
            note: path.clone(),
            text: "kept".into(),
            items: 0,
        }]
    };
    app.planning = handed();
    write(&path, "# Design\n\nkept in mind\n\n- [ ] one\n");
    app.check();
    // One handed on in another note too, which leaves this one's.
    app.planning.push(Planning {
        note: other.clone(),
        text: "elsewhere".into(),
        items: 0,
    });
    // Not in another note, where it waits for its own.
    write(&other, "# Other\n");
    app.open_note(&other, false, false);
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some("elsewhere"));
    app.key(Key::new(KeyCode::Esc, Mods::NONE));
    app.open_note(&path, false, false);
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some("kept"));
    app.key(Key::new(KeyCode::Esc, Mods::NONE));
    // Handed on again, and /plan-item put it in the note: ga starts empty.
    app.planning = handed();
    write(&path, "# Design\n\n- [ ] kept\n- [ ] one\n");
    app.check();
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some(""));
    app.key(Key::new(KeyCode::Esc, Mods::NONE));
    // Handed on, and added while another note was open: done there too.
    app.planning = handed();
    app.open_note(&other, false, false);
    write(&path, "# Design\n\n- [ ] kept\n- [ ] kept\n- [ ] one\n");
    app.open_note(&path, false, false);
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some(""));
}

#[test]
fn an_item_added_after_typing_is_its_own_step_to_undo() {
    let (_dir, mut app) = started_ux2("undo item");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    app.open_note(&path, false, false);
    keys(&mut app, "3GA typed");
    let input = screen(&mut app)
        .iter()
        .position(|l| l.contains("Add an item"))
        .unwrap() as u16;
    click(&mut app, 5, input);
    keys(&mut app, "two<CR>");
    assert_eq!(
        lines(&app),
        ["# Design", "", "- [ ] one typed", "- [ ] two"]
    );
    keys(&mut app, "u");
    assert_eq!(lines(&app), ["# Design", "", "- [ ] one typed"]);
}

#[test]
fn enter_in_a_folded_section_opens_the_fold_not_a_hidden_items_plan() {
    let (_dir, mut app) = started_ux2("folded plan");
    let plan = app.store.create("", Some("Plan - Task"));
    write(&plan, "# Plan - Task\n");
    let path = app.store.create("", Some("Work"));
    write(
        &path,
        "# Work\n## Later\n- [ ] Task\n  Plan: [[Plan - Task]]\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "3G<CR>");
    assert_eq!(name(&app), "Work.md");
    assert!(screen(&mut app).iter().any(|l| l.contains("Task")));
}

#[test]
fn following_a_link_says_where_it_is_going() {
    let (_dir, mut app) = started("open-link");
    app.new_note();
    keys(&mut app, "Links<CR><CR>see https://rfc.example/1<Esc>");
    assert!(app.follow(2, 8));
    if crate::links::says_it() {
        let said = app.message.as_ref().map(|(m, _)| m.as_str()).unwrap_or("");
        assert_eq!(said, "Opening rfc.example in your browser…");
        let said = said.to_string();
        app.link_opened(&said, Ok(false));
        assert!(
            app.message
                .as_ref()
                .unwrap()
                .0
                .ends_with("hasn't shown it yet.")
        );
        app.say(said.clone());
        app.link_opened(&said, Ok(true));
        assert!(app.message.is_none(), "gone once the browser shows it");
    }
}
