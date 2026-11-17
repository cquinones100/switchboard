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
use ratatui::style::Color;

use super::*;
use crate::store::tests::{Dir, set_mtime, temp, write};
use switchboard_ui::input::{Mods, MouseButton, MouseKind};

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
            .any(|r| r.item == Some(Item::Folder(String::new())) && r.text.ends_with(" 1")),
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
    assert!(!app.click_at(2, Some(9)));
    assert!(app.click_at(2, Some(0)));
    assert_eq!(app.editor.doc.line(2), "- [x] eggs #food");
    assert!(app.enter_at(0, 0), "Enter on the title folds its section");
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
    let given = crate::id::of_file(&path).expect("given an ID as it opened");
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
            crate::id::line(&given)
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
            .is_some_and(|(m, _)| m.contains("deleted outside Notes"))
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
    let body = |path: &Path| crate::id::body_of(&fs::read_to_string(path).unwrap());
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
fn opening_a_note_puts_it_first_in_recent_and_a_tag_lists_its_notes() {
    let (dir, mut store) = temp("recent-app");
    let a = store.create("", Some("Groceries"));
    write(&a, "# Groceries\n\n- [ ] eggs #food\n");
    let b = store.create("", Some("Trip"));
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&b, false, false);
    app.open_note(&a, false, false);
    app.open_note(&b, false, false);
    let meta: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path.join(".notes.json")).unwrap()).unwrap();
    assert_eq!(
        meta["recent"],
        serde_json::json!(["Trip.md", "Groceries.md"]),
        "newest first, each once"
    );
    app.open_note(&a, false, false);
    let food = app.editor.doc.line(2).find("#food").unwrap();
    assert!(app.follow(2, food + 1));
    assert_eq!(app.list.tag.as_deref(), Some("food"));
    assert!(
        app.list_rows
            .iter()
            .any(|r| r.text.trim() == "#food    1 note"
                || (r.look == sidebar::Look::Header && r.text.contains("#food")))
    );
    app.focus = Focus::List;
    keys(&mut app, "x");
    assert_eq!(app.list.tag, None, "x clears it");
    assert_eq!(app.list.folder, "");
    app.list_at = app
        .list_rows
        .iter()
        .position(|r| r.item == Some(Item::Recent))
        .unwrap();
    app.activate();
    assert_eq!(app.layers.len(), 1, "Recent's heading opens the history");
    keys(&mut app, "<Esc>");
    assert!(app.layers.is_empty());
    app.list.show(None, None, None);
    keys(&mut app, "/#food<CR>");
    assert_eq!(app.list.tag.as_deref(), Some("food"), "#tag typed into /");
    assert!(
        matches!(
            app.list_rows[app.list_at].look,
            sidebar::Look::Title | sidebar::Look::Open
        ),
        "the cursor on the tag's first note, not one under Recent"
    );
    assert!(app.list_rows[app.list_at].text.contains("Groceries"));
    app.list_at = app
        .list_rows
        .iter()
        .position(|r| matches!(r.item, Some(Item::Note(_))) && r.look == sidebar::Look::Plain)
        .unwrap();
    app.show(None, None, Some("nothing like it".into()));
    assert_eq!(
        app.list_rows[app.list_at].item, None,
        "a search with no notes leaves no Recent row chosen"
    );
    app.show_list(false);
    app.show_list(true);
    assert_eq!(
        app.list_rows[app.list_at].item, None,
        "nor does putting the list away and showing it again"
    );
}

#[test]
fn a_note_moved_to_a_folder_takes_the_list_and_its_cursor_along() {
    let (_dir, mut store) = temp("move-follow");
    store.create("", Some("Stay"));
    let go = store.create("", Some("Go"));
    store.create_folder("Work");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&go, false, false);
    app.focus = Focus::List;
    app.reveal(&go);
    keys(&mut app, "mj<CR>");
    let moved = app.store.absolute("Work/Go.md");
    assert!(moved.is_file());
    assert_eq!(app.list.folder, "Work", "the list follows it");
    assert_eq!(app.list_rows[app.list_at].listed(), Some(moved.as_path()));
}

#[test]
fn deleting_the_last_note_in_recently_deleted_opens_no_live_note() {
    let (_dir, mut store) = temp("trash-last");
    store.create("", Some("Keep"));
    let gone = store.create("", Some("Gone"));
    let (mut app, _) = app_in(store, options());
    app.start();
    let trashed = app.store.delete(&gone).unwrap();
    app.show(Some(store::TRASH.into()), None, None);
    app.open_note(&trashed, true, false);
    app.delete_now(&trashed);
    assert_eq!(app.current_path(), None, "nothing opened in its place");
    assert_eq!(
        app.list.folder,
        store::TRASH,
        "Recently Deleted still shown"
    );
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
    assert_eq!(
        app.request(socket::Request::Open("Work/Plan.md".into())),
        "ok"
    );
    assert_eq!(
        app.list_rows[app.list_at].item,
        Some(Item::Note(work.clone())),
        "the cursor on it"
    );
    assert_eq!(app.list.folder, "Work", "the top folder left for its own");
    assert_eq!(app.request(socket::Request::Shown), "ok Work/Plan.md");
    // The workspace's M-p: the note shown and the commands listed here,
    // the item's first and the workspace's own left out; one run by its
    // words.
    let reply = app.request(socket::Request::Palette);
    let front = socket::InFront::read(reply.strip_prefix("ok ").unwrap()).unwrap();
    assert_eq!(front.note.as_deref(), Some("Work/Plan.md"));
    assert!(front.commands.iter().any(|c| c.name == "Find a note"));
    assert!(
        !front
            .commands
            .iter()
            .any(|c| c.name.starts_with("Move Notes beside")),
        "the workspace lists its own"
    );
    assert_eq!(
        app.request(socket::Request::Run("Find a note".into())),
        "ok"
    );
    assert!(
        app.request(socket::Request::Run("No such thing".into()))
            .starts_with("error")
    );
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

#[test]
fn a_long_note_and_list_have_scroll_bars_that_page_and_drag() {
    let (_dir, mut store) = temp("bars");
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
    let shown = screen(&mut app);
    let bar = app.geo.note_bar.as_ref().map(|b| (b.x, b.y)).unwrap();
    let column: String = shown
        .iter()
        .map(|l| l.chars().nth(bar.0 as usize).unwrap_or(' '))
        .collect();
    assert!(column.contains('┃') && column.contains('│'), "{shown:#?}");
    let list = app.geo.list;
    let list_column: String = shown
        .iter()
        .map(|l| l.chars().nth(list.right() as usize - 1).unwrap_or(' '))
        .collect();
    assert!(list_column.contains('┃'), "{shown:#?}");
    let mouse = |app: &mut App, kind: MouseKind, col: u16, row: u16| {
        app.mouse(Mouse {
            kind,
            button: MouseButton::Left,
            col,
            row,
            mods: Mods::NONE,
        })
    };
    // A press low on the note's track pages it down.
    let bottom = bar.1 + app.geo.note_bar.as_ref().unwrap().height - 1;
    mouse(&mut app, MouseKind::Press, bar.0, bottom);
    screen(&mut app);
    assert!(app.editor.top > 0, "paged down");
    mouse(&mut app, MouseKind::Drag, bar.0, bar.1);
    screen(&mut app);
    assert_eq!(app.editor.top, 0, "dragged back to the top");
    mouse(&mut app, MouseKind::Release, bar.0, bar.1);
    // The list's bar pages the list.
    mouse(
        &mut app,
        MouseKind::Press,
        list.right() - 1,
        list.bottom() - 1,
    );
    mouse(
        &mut app,
        MouseKind::Release,
        list.right() - 1,
        list.bottom() - 1,
    );
    assert!(app.list_top > 0);
    // Off, neither is drawn.
    app.opts.scroll_bars = false;
    screen(&mut app);
    assert!(app.geo.note_bar.is_none());
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
    let added = app.editor.row();
    keys(&mut app, "ggjmajza");
    let folds = app.editor.folds.clone();
    assert_ne!(folds, Default::default(), "a section folded");
    app.open_note(&second, false, false);
    app.open_note(&first, false, false);
    assert_eq!(app.editor.folds, folds, "still folded");
    assert_eq!(app.editor.marks.get('a'), Some((1, 0)), "its mark kept");
    keys(&mut app, "g;");
    assert_eq!(app.editor.row(), added, "and its change found again");
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
fn ctrl_o_and_ctrl_i_go_back_and_forward_through_the_places_jumped_from_across_notes() {
    let (_dir, mut store) = temp("jumps");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\n\none\ntwo\nfind me\nsee [[Beta]]\nlast\n");
    let second = store.create("", Some("Beta"));
    write(&second, "# Beta\n\nbody\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "G/find<CR>");
    assert_eq!(app.editor.row(), 4);
    keys(&mut app, "jfB<CR>");
    assert_eq!(
        app.current_path(),
        Some(second.as_path()),
        "the link followed"
    );
    keys(&mut app, "<C-o>");
    assert_eq!(app.current_path(), Some(first.as_path()));
    assert_eq!(app.editor.row(), 5, "back on the link's line");
    assert_eq!(
        app.editor.marks.context.map(|(r, _)| r),
        Some(5),
        "'' goes back to where it left the note"
    );
    keys(&mut app, "<C-o>");
    assert_eq!(app.editor.row(), 6, "back where the search started");
    keys(&mut app, "2<C-i>");
    assert_eq!(
        app.current_path(),
        Some(second.as_path()),
        "forward to Beta"
    );
    let at = (app.editor.row(), app.editor.col());
    keys(&mut app, "<Tab>");
    assert_eq!(app.current_path(), Some(second.as_path()));
    assert_eq!((app.editor.row(), app.editor.col()), at, "nothing forward");
}

/// Alpha with the cursor on its fourth line, ready for Beta to be opened.
fn in_alpha(name: &str) -> (Dir, App, PathBuf, PathBuf) {
    let (dir, mut store) = temp(name);
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\n\none\ntwo\nthree\n");
    let second = store.create("", Some("Beta"));
    write(&second, "# Beta\n\nbody\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "3j");
    (dir, app, first, second)
}

/// Ctrl-O from Beta goes back to Alpha's fourth line, and Ctrl-I to Beta.
fn back_and_forth(app: &mut App, first: &Path, second: &Path, way: &str) {
    assert_eq!(app.current_path(), Some(second), "{way}: Beta opened");
    keys(app, "<C-o>");
    assert_eq!(app.current_path(), Some(first), "{way}: back to Alpha");
    assert_eq!(app.editor.row(), 3, "{way}: on the line it left");
    keys(app, "<C-i>");
    assert_eq!(app.current_path(), Some(second), "{way}: forward to Beta");
}

#[test]
fn ctrl_o_and_ctrl_i_move_between_notes_however_the_second_was_opened() {
    let (_dir, mut app, first, second) = in_alpha("jumps-list");
    app.show(Some(String::new()), None, None);
    app.list_at = app
        .list_rows
        .iter()
        .position(|r| r.item == Some(Item::Note(second.clone())))
        .unwrap();
    app.focus = Focus::List;
    app.activate();
    back_and_forth(&mut app, &first, &second, "the list");

    let (_dir, mut app, first, second) = in_alpha("jumps-finder");
    keys(&mut app, "<C-f>Beta<CR>");
    back_and_forth(&mut app, &first, &second, "the finder");

    let (_dir, mut app, first, second) = in_alpha("jumps-outside");
    assert_eq!(app.request(socket::Request::Open("Beta.md".into())), "ok");
    back_and_forth(&mut app, &first, &second, "from outside");
}

#[test]
fn ctrl_o_goes_back_to_a_note_opened_before_notes_started_again() {
    let (dir, mut app, first, second) = in_alpha("jumps-kept");
    app.open_note(&second, false, false);
    app.stop();
    let mut store = Store::at(&dir.path);
    store.state = dir.path.join(".state-for-test");
    let (mut again, _) = app_in(store, options());
    again.start();
    assert_eq!(
        again.current_path(),
        Some(second.as_path()),
        "Beta, open last"
    );
    back_and_forth(&mut again, &first, &second, "started again");
}

#[test]
fn notes_started_again_on_another_tabs_note_goes_back_to_where_it_ended() {
    let (dir, mut app, first, second) = in_alpha("jumps-ended");
    app.stop();
    let mut store = Store::at(&dir.path);
    store.state = dir.path.join(".state-for-test");
    // The workspace opening Notes beside a tab whose note is Beta.
    store.set_last(Some("Beta.md"));
    let (mut again, _) = app_in(store, options());
    again.start();
    back_and_forth(&mut again, &first, &second, "another tab's note");
}

#[test]
fn a_jumps_line_deleted_and_another_put_in_its_place_is_not_gone_back_to() {
    let (_dir, mut store) = temp("jumps-replaced");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\nB\nC\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "jGkddOX<Esc>j<C-o>");
    assert_eq!(app.editor.line(), "C", "B's entry went with it");
}

#[test]
fn a_mark_whose_line_went_from_outside_is_not_taken_by_a_new_line() {
    let (_dir, mut store) = temp("mark-outside");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\nA\nB\nC\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "jjma");
    write(&first, "# Alpha\nA\nC\n");
    app.check();
    keys(&mut app, "OX<Esc>gg`a");
    assert_eq!(app.editor.row(), 0, "the mark went with B");
}

#[test]
fn two_outside_changes_in_a_row_are_followed_each_on_its_own() {
    let (_dir, mut store) = temp("mark-outside-twice");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\nA\nB\nC\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "jjmagg");
    write(&first, "# Alpha\nA\nC\n");
    app.check();
    write(&first, "# Alpha\nA\nX\nC\n");
    app.check();
    keys(&mut app, "`a");
    assert_eq!(app.editor.row(), 0, "the mark went with B");
}

#[test]
fn insert_modes_ctrl_o_then_a_jump_goes_back_to_typing() {
    let (_dir, mut store) = temp("jumps-insert");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\none\ntwo\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "jGi<C-o>1<C-o>x");
    assert_eq!(app.editor.mode, Mode::Insert);
    assert_eq!(app.editor.line(), "xone", "x typed where it went back to");
    keys(&mut app, "<Esc>Gi<C-o><C-o>y");
    assert_eq!(app.editor.mode, Mode::Insert);
    assert_eq!(app.editor.row(), 1, "an uncounted one goes back too");
    keys(&mut app, "<Esc>j.");
    assert_eq!(
        app.editor.line(),
        "ytwo",
        ". types what was typed after the jump"
    );
    keys(&mut app, "xiW<C-o><C-o><Esc>j0.");
    assert!(
        app.editor.line().starts_with('W'),
        ". types what was typed before a jump with nothing after it: {}",
        app.editor.line()
    );
    let second = app.store.create("", Some("Beta"));
    write(&second, "# Beta\n");
    keys(&mut app, "<Esc>");
    app.open_note(&second, false, false);
    keys(&mut app, "i<C-o>1<C-o>z");
    assert_eq!(app.current_path(), Some(first.as_path()));
    assert_eq!(
        app.editor.mode,
        Mode::Insert,
        "typing goes on in the note gone to"
    );
}

#[test]
fn a_mark_on_a_line_dot_deletes_goes_with_it() {
    let (_dir, mut store) = temp("mark-dot");
    let first = store.create("", Some("Alpha"));
    write(&first, "# T\nA\nB\nC\nD\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "ji<C-o>ddX<Esc>jma.gg`a");
    assert_eq!(app.editor.row(), 0, "the mark went with C");
}

#[test]
fn a_mark_on_an_item_goes_with_it_as_ticking_sorts_it_down() {
    let (_dir, mut store) = temp("mark-tick");
    let first = store.create("", Some("Alpha"));
    write(&first, "# T\n\n- [ ] alpha\n- [ ] beta\n- [ ] gamma\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "jjma<CR>gg'a");
    assert!(
        app.editor.line().ends_with("alpha"),
        "{}",
        app.editor.line()
    );
}

#[test]
fn a_table_lined_up_as_the_cursor_moves_into_it_is_no_change_to_visit() {
    let (_dir, mut store) = temp("table-tidy");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\n\none\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    write(
        &first,
        "# Alpha\n\none\n| a | bb |\n|---|---|\n| ccc | d |\n",
    );
    app.check();
    keys(&mut app, "Gg;");
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("E664")),
        "the change list is empty: {:?}",
        app.message
    );
}

#[test]
fn ctrl_o_back_to_a_note_changed_meanwhile_finds_the_line_where_it_went() {
    let (_dir, mut store) = temp("jumps-moved");
    let first = store.create("", Some("Alpha"));
    write(&first, "# Alpha\n\none\nsee [[Beta]]\n");
    let second = store.create("", Some("Beta"));
    write(&second, "# Beta\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&first, false, false);
    keys(&mut app, "4GfB<CR>");
    assert_eq!(app.current_path(), Some(second.as_path()));
    write(&first, "# Alpha\n\nnew\nlines\none\nsee [[Beta]]\n");
    keys(&mut app, "<C-o>");
    assert_eq!(app.current_path(), Some(first.as_path()));
    assert_eq!(
        app.editor.line(),
        "see [[Beta]]",
        "on the link's line as it moved"
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
fn r_types_over_in_replace_mode_and_dot_does_it_again() {
    let (_dir, mut app) = started("replace-mode");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "hello world"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "R");
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("-- REPLACE --")),
        "{}",
        shown.join("\n")
    );
    keys(&mut app, "ab<Esc>");
    assert_eq!(app.editor.doc.line(2), "abllo world");
    keys(&mut app, "w.");
    assert_eq!(app.editor.doc.line(2), "abllo abrld", ". types over again");
    keys(&mut app, "0j");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "0123456789", "0123456789"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "3R");
    app.paste("ab");
    keys(&mut app, "<Esc>");
    assert_eq!(
        app.editor.doc.line(2),
        "ababab6789",
        "a paste typed again for a count"
    );
    keys(&mut app, "j0.");
    assert_eq!(
        app.editor.doc.line(3),
        "ababab6789",
        ". types the paste over again"
    );
}

#[test]
fn ctrl_o_runs_one_command_and_types_on_in_the_same_item() {
    let (_dir, mut app) = started("ctrl-o");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "- [ ] one"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "A <C-o>");
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("-- (insert) --")),
        "{}",
        shown.join("\n")
    );
    keys(&mut app, "0two<Esc>");
    assert_eq!(app.editor.doc.line(2), "two- [ ] one ");
    keys(&mut app, "u");
    assert_eq!(
        app.editor.doc.line(2),
        "- [ ] one ",
        "what was typed after Ctrl-O is a step of its own"
    );
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "a"]));
    app.editor.doc.row = 2;
    keys(&mut app, "0i<C-o>/<BS>x");
    assert_eq!(
        app.editor.doc.line(2),
        "xa",
        "Backspace on an empty prompt goes back to typing"
    );
    keys(&mut app, "<Esc>u");
    keys(&mut app, "A<C-o>:s/a//<CR>");
    assert_eq!(
        (app.editor.doc.line(2), app.editor.mode),
        (String::new(), Mode::Insert),
        "an Ex command under Ctrl-O runs, then typing goes on"
    );
    keys(&mut app, "<Esc>");
    keys(&mut app, ".");
    assert!(app.prompt.is_none(), ". opens no prompt left over from it");
}

#[test]
fn ctrl_v_highlights_the_same_columns_on_every_row_of_the_block() {
    let (_dir, mut app) = started("block-visual");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "abcdefgh", "abcdefgh"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 2;
    keys(&mut app, "<C-v>jl");
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut buf);
    let shown: Vec<String> = (0..buf.area.height)
        .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
        .collect();
    assert!(shown.iter().any(|l| l.contains("-- VISUAL BLOCK --")));
    let (x, y) = find(&shown, "abcdefgh");
    for row in [y, y + 1] {
        let bg = |dx: u16| buf[(x + dx, row)].bg;
        assert_eq!(bg(2), bg(3), "c and d alike on row {row}");
        assert_ne!(bg(1), bg(2), "b outside the block on row {row}");
        assert_ne!(bg(4), bg(3), "e outside the block on row {row}");
    }
    assert_eq!(buf[(x + 2, y)].bg, buf[(x + 2, y + 1)].bg);
}

#[test]
fn a_style_from_the_format_menu_in_replace_mode_is_for_what_is_typed_next() {
    let (_dir, mut app) = started("replace-format");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "hello"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "RABCDE<C-p>b");
    assert_eq!(app.editor.doc.line(2), "ABCDE****", "as in Insert mode");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "hello"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "<Esc>");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "ab"]));
    app.editor.doc.col = 0;
    keys(&mut app, "2RAB<C-p>b");
    assert_eq!(app.editor.doc.line(2), "ABAB****", "after R's count");
}

#[test]
fn tab_in_a_list_moves_the_column_down_keeps() {
    let (_dir, mut app) = started("list-want");
    let row = "- one two three";
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", row, row, row]));
    app.editor.doc.row = 2;
    app.editor.insert_at(4);
    let down = Key::new(KeyCode::Down, Mods::NONE);
    app.key(down);
    keys(&mut app, "<Tab>");
    let col = app.editor.doc.col;
    app.key(down);
    assert_eq!(
        (app.editor.doc.row, app.editor.doc.col),
        (4, col),
        "under where Tab left the cursor"
    );
}

#[test]
fn a_count_before_dot_replaces_the_changes_count() {
    let (_dir, mut app) = started("dot-count");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "one two three four five six"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "dw2.");
    assert_eq!(app.editor.doc.line(2), "four five six");
    keys(&mut app, ".");
    assert_eq!(
        app.editor.doc.line(2),
        "six",
        "the count kept for the next ."
    );
    keys(&mut app, "u");
    assert_eq!(app.editor.doc.line(2), "four five six", "one step each");
    keys(&mut app, "u");
    assert_eq!(app.editor.doc.line(2), "two three four five six");
}

#[test]
fn a_count_before_dot_keeps_an_insert_after_the_lists_marker() {
    let (_dir, mut app) = started("dot-count-insert");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "- [ ] one", "- [ ] two"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "I!<Esc>j2.");
    assert_eq!(app.editor.doc.line(2), "- [ ] !one");
    assert_eq!(app.editor.doc.line(3), "- [ ] !two");
}

#[test]
fn dot_adds_again_after_ctrl_a() {
    let (_dir, mut app) = started("increment");
    app.editor
        .change(|doc| doc.text = Text::from_lines(&["# T", "", "3 cups"]));
    app.editor.doc.row = 2;
    app.editor.doc.col = 0;
    keys(&mut app, "2<C-a>");
    assert_eq!(app.editor.doc.line(2), "5 cups");
    keys(&mut app, ".");
    assert_eq!(app.editor.doc.line(2), "7 cups", ". adds the same again");
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
    keys(&mut app, "<Esc><C-S-p>Show or put away the sidebar<CR>");
    assert!(!app.list_shown, "the list put away from a note");
    keys(&mut app, "<C-S-p>");
    let shown = screen(&mut app);
    assert!(
        shown
            .iter()
            .any(|l| l.contains("Show or put away the sidebar") && l.contains("Ctrl-B")),
        "the one picked last first, with its key"
    );
    keys(&mut app, "<Esc>");
    let recent = palette::recent(&app.recent_file);
    assert_eq!(recent[0], "Show or put away the sidebar");
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
fn line_numbers_are_each_lines_own_unless_relative_numbers_is_on() {
    let (_dir, mut store) = temp("absolute-numbers");
    let path = store.create("", Some("Nums"));
    write(&path, "# Nums\n\n## Part\na\nb\n## Next\nc\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    keys(&mut app, "3G");
    let gutter = |app: &mut App, rows: usize| {
        let shown = screen(app);
        let (x, y) = (app.geo.note.x as usize, app.geo.text.y as usize);
        (y..y + rows)
            .map(|y| shown[y].chars().skip(x).take(3).collect::<String>())
            .map(|n| n.trim().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        gutter(&mut app, 7),
        ["1", "2", "3", "4", "5", "6", "7"],
        "off, every line shows its own number"
    );
    app.opts.relative_numbers = true;
    assert_eq!(
        gutter(&mut app, 7),
        ["2", "1", "3", "1", "2", "3", "4"],
        "on, the distance from the cursor's line, which shows its own"
    );
    keys(&mut app, "zc");
    assert_eq!(
        gutter(&mut app, 5),
        ["2", "1", "3", "3", "4"],
        "a closed fold's hidden lines are counted"
    );
    app.opts.relative_numbers = false;
    assert_eq!(gutter(&mut app, 5), ["1", "2", "3", "6", "7"]);
}

#[test]
fn the_note_window_numbers_its_lines_relative_to_the_cursor() {
    let (_dir, mut app) = started("numbers");
    app.opts.relative_numbers = true;
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
    use crate::id;
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
        crate::store::mtime(&old),
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
        crate::store::title_of("\n<!-- note-id: n-0123456789ab -->\n"),
        "New Note"
    );
    assert_eq!(
        crate::store::preview_of("# T\n\nwords\n\n<!-- note-id: n-0123456789ab -->\n"),
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
    use crate::id;
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
    let kinds = crate::render::kinds(&["# T", "```", "code", line]);
    assert_eq!(
        kinds[3],
        crate::render::Kind::Id,
        "hidden inside an unclosed code block"
    );
    let mut doc = crate::format::Doc::new(Text::from_lines(&["# T", "one", line]));
    crate::format::paragraph(&mut doc, 1, 2, crate::format::Paragraph::Check);
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
            detail: "review round 1".into(),
            color: None,
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
            detail: "git push".into(),
            color: None,
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
        switchboard_ui::kit::Surface::Sunken.color(&theme)
    );
    // Behind it, dimmed.
    assert_ne!(buf[(0, 0)].style(), before[(0, 0)].style());
    assert_eq!(buf[(outer.right(), outer.y + 1)].symbol(), "▌");
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
    assert!(footer.contains("Enter tick or follow"), "{footer}");
    assert!(footer.contains("M-p commands, autopilot"), "{footer}");
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
    // A command run from the workspace's M-p leaves the input, so the
    // keys after it reach what it went to; Add an item itself keeps it.
    keys(&mut app, "gadraft");
    assert_eq!(
        app.request(socket::Request::Run("Add an item".into())),
        "ok"
    );
    assert_eq!(app.adding.as_deref(), Some("draft"));
    assert_eq!(
        app.request(socket::Request::Run("Go to the sidebar".into())),
        "ok"
    );
    assert_eq!(app.adding, None);
    // So do the finder and a search being typed.
    keys(&mut app, "<C-f>");
    assert!(app.top_kind().is_some());
    assert_eq!(
        app.request(socket::Request::Run("Clear the search or tag".into())),
        "ok"
    );
    assert!(app.top_kind().is_none());
    app.focus = Focus::Note;
    keys(&mut app, "/sear");
    assert!(app.prompt.is_some());
    assert_eq!(
        app.request(socket::Request::Run("Clear the search or tag".into())),
        "ok"
    );
    assert!(app.prompt.is_none());
}

#[test]
fn enter_on_an_item_ticks_it_and_on_its_plan_link_shows_the_plan_beside() {
    let (_dir, mut app) = started_ux2("plan");
    let plan = app.store.create("", Some("Plan - Kit"));
    write(&plan, "# Plan - Kit\n\nThe steps\n");
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] planned\n  Plan: [[Plan - Kit]]\n- [ ] bare\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "4G$<CR>");
    assert_eq!(name(&app), "Design.md");
    assert_eq!(
        app.beside.as_ref().map(|b| b.plan.name.as_str()),
        Some("Kit")
    );
    assert!(screen(&mut app).iter().any(|l| l.contains("The steps")));
    keys(&mut app, "<Esc>");
    assert!(app.beside.is_none());
    keys(&mut app, "3G<CR>");
    assert!(app.beside.is_none());
    assert!(
        lines(&app).iter().any(|l| l == "- [x] planned"),
        "{:?}",
        lines(&app)
    );
    // o with no tab on the item is Vim's.
    keys(&mut app, "Go");
    assert_eq!(app.editor.mode, Mode::Insert);
}

#[test]
fn enter_on_a_link_in_an_item_follows_it_and_elsewhere_on_the_line_ticks_it() {
    let (_dir, mut app) = started_ux2("item link");
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] read https://rfc.example/1 first\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "3G0fh<CR>");
    assert_eq!(app.opened, ["https://rfc.example/1"]);
    assert_eq!(lines(&app)[2], "- [ ] read https://rfc.example/1 first");
    keys(&mut app, "$<CR>");
    assert_eq!(lines(&app)[2], "- [x] read https://rfc.example/1 first");
    assert_eq!(app.opened.len(), 1);
}

#[test]
fn enter_just_left_of_an_image_opens_it() {
    let (_dir, mut app) = started("image enter");
    let path = app.store.create("", Some("Shots"));
    write(&path.with_file_name("shot.png"), "\u{89}PNG");
    write(
        &path,
        "# Shots\n\n- [ ] the button\n  ![shot](shot.png)\n- [ ] gone ![gone](missing.png)\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "4G0l<CR>");
    assert_eq!(app.opened, ["shot.png"]);
    assert_eq!(lines(&app)[2], "- [ ] the button");
    // One that isn't there is said, and the item stays unticked.
    keys(&mut app, "5G$<CR>");
    assert_eq!(app.opened.len(), 1);
    assert_eq!(lines(&app)[4], "- [ ] gone ![gone](missing.png)");
    assert_eq!(
        app.message.as_ref().map(|m| m.0.as_str()),
        Some("notes: no picture at missing.png")
    );
}

#[test]
fn enter_on_a_folded_heading_follows_its_link_and_elsewhere_unfolds() {
    let (_dir, mut app) = started("folded link");
    let path = app.store.create("", Some("Work"));
    write(
        &path,
        "# Work\n\n## See https://rfc.example/2\n\nhidden\n\n## Plain\n\ntext\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "3G0<CR>");
    let kinds = render::kinds(&lines(&app));
    assert!(app.editor.folds.closed_at(&kinds, 2).is_some(), "folded");
    keys(&mut app, "3G0fh<CR>");
    assert_eq!(app.opened, ["https://rfc.example/2"]);
    assert!(
        app.editor.folds.closed_at(&kinds, 2).is_some(),
        "still folded"
    );
    keys(&mut app, "3G0<CR>");
    assert!(app.editor.folds.closed_at(&kinds, 2).is_none(), "unfolded");
}

#[test]
fn enter_on_a_tag_shows_its_notes() {
    let (_dir, mut app) = started("tag enter");
    let path = app.store.create("", Some("Groceries"));
    write(&path, "# Groceries\n\n- [ ] eggs #food\n");
    app.open_note(&path, false, false);
    keys(&mut app, "3G$<CR>");
    assert_eq!(app.list.tag.as_deref(), Some("food"));
    assert_eq!(lines(&app)[2], "- [ ] eggs #food");
}

#[test]
fn enter_on_a_tag_in_bold_shows_its_notes_rather_than_ticking() {
    let (_dir, mut app) = started("bold tag enter");
    let path = app.store.create("", Some("Groceries"));
    write(&path, "# Groceries\n\n- [ ] **buy #food today**\n");
    app.open_note(&path, false, false);
    keys(&mut app, "3G0f#<CR>");
    assert_eq!(app.list.tag.as_deref(), Some("food"));
    assert_eq!(lines(&app)[2], "- [ ] **buy #food today**");
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
        color: None,
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

// Board 7's chips at the right of each item.
fn chips_note(app: &mut App) -> std::path::PathBuf {
    let plan = app.store.create("", Some("Plan - Logo"));
    write(&plan, "# Plan - Logo\n\nDraw it.\n");
    let path = app.store.create("", Some("Design"));
    let long = "word ".repeat(30);
    write(
        &path,
        &format!(
            "# Design\n\n- [ ] Draw the logo {long}end\n  Plan: [[Plan - Logo]]\n- [ ] Short one\n\n- [x] Pick a name\n  Done: 2505ca4f, already on main\n"
        ),
    );
    app.item_tabs = HashMap::from([(
        note_item_key("Design.md", &format!("Draw the logo {long}end")),
        render::ItemTab {
            id: "@7".into(),
            note: "Design.md".into(),
            note_id: String::new(),
            index: 7,
            waits: false,
            detail: "review round 1".into(),
            color: Some(Color::Rgb(0x9a, 0xd0, 0xf5)),
        },
    )]);
    app.open_note(&path, false, false);
    path
}

#[test]
fn an_items_chips_take_its_plan_lines_place_and_sit_at_its_right_while_the_cursor_is_on_that_line()
{
    let (_dir, mut app) = started_ux2("item chips");
    chips_note(&mut app);
    app.editor.doc.row = 5;
    let shown = screen(&mut app);
    let at = shown
        .iter()
        .position(|l| l.contains("Draw the logo"))
        .unwrap();
    assert!(!shown[at].contains('…'), "wrapped whole: {:?}", shown[at]);
    let end = shown[at..].iter().position(|l| l.contains("end")).unwrap() + at;
    let under = shown[end + 1].trim_end();
    assert!(
        under.ends_with("  ⠹ tab 7 · review round 1   plan"),
        "the tab and the plan under it, at the plan line's indent: {under:?}"
    );
    assert!(
        !shown.iter().any(|l| l.contains("Plan:")),
        "the plan line's text not shown: {shown:#?}"
    );
    let ticked = shown.iter().find(|l| l.contains("Pick a name")).unwrap();
    assert!(ticked.ends_with("landed 2505ca4"), "{ticked:?}");
    let short = shown.iter().find(|l| l.contains("Short one")).unwrap();
    assert!(
        short.trim_end().ends_with("Short one"),
        "no chips: {short:?}"
    );
    // The tab's number in its color.
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut buf);
    let (x, y) = find(&shown, "tab 7");
    assert_eq!(buf[(x, y)].fg, Color::Rgb(0x9a, 0xd0, 0xf5));

    // The cursor on the plan line: its text to edit, the chips at the
    // item's right, its text cut before them.
    app.editor.doc.row = 3;
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("Plan: Plan - Logo")),
        "{shown:#?}"
    );
    let item = shown.iter().find(|l| l.contains("Draw the logo")).unwrap();
    assert!(
        item.ends_with("⠹ tab 7 · review round 1   plan"),
        "the tab and the plan at the right edge: {item:?}"
    );
    assert!(item.contains("word…  ⠹"), "{item:?}");

    // Off, the items are their text alone.
    app.opts.item_chips = false;
    app.editor.doc.row = 5;
    let shown = screen(&mut app);
    assert!(
        !shown
            .iter()
            .any(|l| l.contains("tab 7") || l.contains("landed"))
    );
    assert!(shown.iter().any(|l| l.contains("Plan: Plan - Logo")));
}

#[test]
fn a_plan_lines_choice_is_a_chip_under_its_item() {
    let (_dir, mut app) = started_ux2("choice chips");
    let path = app.store.create("", Some("Choices"));
    write(
        &path,
        "# Choices\n\n- [ ] A condensed sidebar\n  - Seen in the audit\n  Plan: [[Plan - Condensed]] (needs a choice: A, B)\n- [ ] Stable IDs\n  Plan: [[Plan - IDs]] (chose A)\n",
    );
    app.open_note(&path, false, false);
    app.editor.doc.row = 1;
    let shown = screen(&mut app);
    assert!(
        shown
            .iter()
            .any(|l| l.trim_end().ends_with("  needs a choice: A, B   plan")),
        "{shown:#?}"
    );
    assert!(
        shown
            .iter()
            .any(|l| l.trim_end().ends_with("  plan   chose: A")),
        "{shown:#?}"
    );
    assert!(
        shown.iter().any(|l| l.contains("Seen in the audit")),
        "the other lines under it as they are"
    );
}

#[test]
fn the_chip_under_the_mouse_is_lit_wherever_the_chips_are_drawn() {
    let (_dir, mut app) = started_ux2("chip hover");
    chips_note(&mut app);
    app.editor.doc.row = 5;
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "plan");
    app.mouse(Mouse {
        kind: MouseKind::Move,
        button: MouseButton::None,
        col: x,
        row: y,
        mods: Mods::NONE,
    });
    let lit = |app: &mut App| {
        let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
        app.draw(&mut buf);
        buf[(x, y)].bg == Color::Indexed(81)
    };
    assert!(lit(&mut app), "the plan chip under the mouse");
    // The cursor onto the plan line: its chips go to the item's right,
    // away from the mouse, and none of them is lit.
    app.editor.doc.row = 3;
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut buf);
    assert!(!buf.content.iter().any(|c| c.bg == Color::Indexed(81)));
}

#[test]
fn a_click_on_an_items_plan_chip_opens_its_plan_and_on_its_tab_chip_goes_to_the_tab() {
    let (_dir, mut app) = started_ux2("chip clicks");
    chips_note(&mut app);
    // Typed into another item, still in Insert mode.
    keys(&mut app, "5GA again");
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "tab 7");
    click(&mut app, x, y);
    assert_eq!(app.editor.doc.row, 2, "the cursor on its item");
    assert_eq!(
        app.message.as_ref().map(|m| m.0.as_str()),
        Some("notes: that needs Switchboard's workspace"),
        "as o goes to the tab, outside the workspace"
    );
    keys(&mut app, "u");
    assert_eq!(
        lines(&app)[4],
        "- [ ] Short one",
        "what was typed one step to undo"
    );
    // And the change for . to make again.
    keys(&mut app, "5G.");
    assert_eq!(lines(&app)[4], "- [ ] Short one again");
    app.editor.doc.row = 5;
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "plan");
    click(&mut app, x, y);
    assert_eq!(
        app.beside.as_ref().map(|b| b.plan.name.as_str()),
        Some("Logo"),
        "the plan shown beside the note, as Enter"
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
            note_id: None,
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
        note_id: None,
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
    assert!(app.beside.is_none());
    assert!(screen(&mut app).iter().any(|l| l.contains("Task")));
}

#[test]
fn following_a_link_says_where_it_is_going() {
    let (_dir, mut app) = started("open-link");
    app.new_note();
    keys(&mut app, "Links<CR><CR>see https://rfc.example/1<Esc>");
    assert!(app.follow(2, 8));
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
    // A link that couldn't be opened says why, whatever was said before.
    app.link_opened("", Err("nothing here opens a link".into()));
    assert_eq!(
        app.message.as_ref().unwrap().0,
        "Couldn't open it: nothing here opens a link"
    );
}

#[test]
fn a_note_renamed_outside_is_followed_by_its_id() {
    let (dir, mut store) = temp("renamed-outside");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let given = crate::id::of_file(&path).expect("given an ID as it opened");
    // Renamed, and its title changed with it, from outside.
    let moved = dir.path.join("Roadmap.md");
    fs::rename(&path, &moved).unwrap();
    write(
        &moved,
        &format!("# Roadmap\n\n- [ ] one\n\n{}\n", crate::id::line(&given)),
    );
    app.handle(Msg::Watched);
    app.watch_due = Some(Instant::now());
    app.tick();
    assert_eq!(name(&app), "Roadmap.md");
    assert_eq!(lines(&app).join("\n"), "# Roadmap\n\n- [ ] one");
    assert!(app.message.is_none(), "nothing said about a deleted note");
    assert_eq!(app.store.last(), Some(moved.clone()), "the last note open");
    let shown = screen(&mut app).join("\n");
    assert_eq!(
        shown.matches("Roadmap").count(),
        3,
        "listed once, and once under Recent: {shown}"
    );
    assert!(!shown.contains("Plan"), "{shown}");
    // Typing saves to the new file, never the old one.
    app.editor
        .change(|doc| doc.text.set_line(2, "- [ ] one, typed"));
    app.save();
    assert!(!path.exists());
    assert!(
        fs::read_to_string(&moved)
            .unwrap()
            .contains("- [ ] one, typed")
    );
}

#[test]
fn typing_not_yet_saved_merges_into_a_note_renamed_outside() {
    let (dir, mut store) = temp("renamed-typing");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n- [ ] two\n- [ ] three\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let given = crate::id::of_file(&path).unwrap();
    app.editor
        .change(|doc| doc.text.set_line(2, "- [ ] one, typed"));
    let folder = dir.path.join("Work");
    fs::create_dir_all(&folder).unwrap();
    let moved = folder.join("Plan.md");
    fs::rename(&path, &moved).unwrap();
    write(
        &moved,
        &format!(
            "# Plan\n\n- [ ] one\n- [ ] two\n- [ ] three (ABC-2)\n\n{}\n",
            crate::id::line(&given)
        ),
    );
    app.save();
    assert!(!path.exists(), "nothing written at the old path");
    assert_eq!(
        fs::read_to_string(&moved).unwrap(),
        format!(
            "# Plan\n\n- [ ] one, typed\n- [ ] two\n- [ ] three (ABC-2)\n\n{}\n",
            crate::id::line(&given)
        )
    );
    assert_eq!(app.current_path(), Some(moved.as_path()));
}

#[test]
fn a_note_moved_to_recently_deleted_outside_stays_unsaved_on_screen() {
    let (dir, mut store) = temp("trashed-outside");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let trashed = dir.path.join(".trash").join("Plan.md");
    fs::create_dir_all(trashed.parent().unwrap()).unwrap();
    fs::rename(&path, &trashed).unwrap();
    let in_trash = fs::read_to_string(&trashed).unwrap();
    app.editor
        .change(|doc| doc.text.set_line(2, "- [ ] one, typed"));
    app.save();
    assert_eq!(app.current_path(), Some(path.as_path()));
    assert_eq!(fs::read_to_string(&trashed).unwrap(), in_trash);
    assert!(!path.exists());
    assert_eq!(lines(&app)[2], "- [ ] one, typed");
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("was deleted outside Notes"))
    );
}

#[test]
fn a_note_renamed_outside_beside_a_copy_holding_its_id_is_followed_not_the_copy() {
    let (dir, mut store) = temp("renamed-copied");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let copy = dir.path.join("Backup.md");
    fs::copy(&path, &copy).unwrap();
    let copy_was = fs::read_to_string(&copy).unwrap();
    let moved = dir.path.join("Roadmap.md");
    fs::rename(&path, &moved).unwrap();
    app.check();
    assert_eq!(app.current_path(), Some(moved.as_path()));
    app.editor
        .change(|doc| doc.text.set_line(2, "- [ ] one, typed"));
    app.save();
    assert_eq!(fs::read_to_string(&copy).unwrap(), copy_was);
    let saved = app.current_path().unwrap().to_path_buf();
    assert_ne!(saved, copy);
    assert!(
        fs::read_to_string(&saved)
            .unwrap()
            .contains("- [ ] one, typed")
    );
}

#[test]
fn a_note_deleted_outside_beside_a_copy_holding_its_id_stays_unsaved() {
    let (dir, mut store) = temp("deleted-copied");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let copy = dir.path.join("Backup.md");
    fs::copy(&path, &copy).unwrap();
    let copy_was = fs::read_to_string(&copy).unwrap();
    fs::remove_file(&path).unwrap();
    app.editor
        .change(|doc| doc.text.set_line(2, "- [ ] one, typed"));
    app.save();
    assert_eq!(app.current_path(), Some(path.as_path()));
    assert_eq!(fs::read_to_string(&copy).unwrap(), copy_was);
    assert!(!path.exists());
    assert_eq!(lines(&app)[2], "- [ ] one, typed");
}

#[test]
fn an_item_handed_to_plan_item_follows_its_note_renamed_outside() {
    let (dir, mut app) = started_ux2("plan renamed");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    app.open_note(&path, false, false);
    app.planning = vec![Planning {
        note: path.clone(),
        note_id: None,
        text: "kept".into(),
        items: 0,
    }];
    let moved = dir.path.join("Look.md");
    fs::rename(&path, &moved).unwrap();
    app.check();
    assert_eq!(app.current_path(), Some(moved.as_path()));
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some("kept"));
}

#[test]
fn an_item_handed_to_plan_item_finds_its_note_renamed_outside_while_closed() {
    let (dir, mut app) = started_ux2("plan renamed closed");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    let other = app.store.create("", Some("Other"));
    write(&other, "# Other\n");
    app.open_note(&path, false, false);
    app.planning = vec![Planning {
        note: path.clone(),
        note_id: app.open.as_ref().unwrap().id.clone(),
        text: "kept".into(),
        items: 0,
    }];
    app.open_note(&other, false, false);
    let moved = dir.path.join("Look.md");
    fs::rename(&path, &moved).unwrap();
    app.open_note(&moved, false, false);
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some("kept"));
}

#[test]
fn a_note_deleted_outside_beside_a_copy_holding_its_id_is_not_followed() {
    let (dir, mut store) = temp("trashed-copied");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let copy = dir.path.join("Backup.md");
    fs::copy(&path, &copy).unwrap();
    let copy_was = fs::read_to_string(&copy).unwrap();
    let trashed = dir.path.join(".trash").join("Plan.md");
    fs::create_dir_all(trashed.parent().unwrap()).unwrap();
    fs::rename(&path, &trashed).unwrap();
    app.editor
        .change(|doc| doc.text.set_line(2, "- [ ] one, typed"));
    app.save();
    assert_eq!(app.current_path(), Some(path.as_path()));
    assert_eq!(fs::read_to_string(&copy).unwrap(), copy_was);
    assert!(!path.exists());
}

#[test]
fn a_note_saved_into_a_new_file_unchanged_and_then_renamed_is_followed() {
    let (dir, mut store) = temp("replaced-renamed");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    // Another editor's save: a new file put in its place, the same text.
    let text = fs::read_to_string(&path).unwrap();
    let beside = dir.path.join(".Plan.md.tmp");
    write(&beside, &text);
    fs::rename(&beside, &path).unwrap();
    app.check();
    let moved = dir.path.join("Roadmap.md");
    fs::rename(&path, &moved).unwrap();
    app.check();
    assert_eq!(app.current_path(), Some(moved.as_path()));
}

#[test]
fn an_item_handed_to_plan_item_finds_its_note_saved_elsewhere_then_renamed() {
    let (dir, mut app) = started_ux2("plan saved renamed");
    let path = app.store.create("", Some("Design"));
    write(&path, "# Design\n\n- [ ] one\n");
    let other = app.store.create("", Some("Other"));
    write(&other, "# Other\n");
    app.open_note(&path, false, false);
    app.planning = vec![Planning {
        note: path.clone(),
        note_id: app.open.as_ref().unwrap().id.clone(),
        text: "kept".into(),
        items: 0,
    }];
    app.open_note(&other, false, false);
    // Another editor's save, into a new file, while it is closed.
    let text = fs::read_to_string(&path).unwrap();
    let beside = dir.path.join(".Design.md.tmp");
    write(&beside, &text);
    fs::rename(&beside, &path).unwrap();
    app.check();
    let moved = dir.path.join("Look.md");
    fs::rename(&path, &moved).unwrap();
    app.open_note(&moved, false, false);
    keys(&mut app, "ga");
    assert_eq!(app.adding.as_deref(), Some("kept"));
}

#[test]
fn a_note_moved_to_another_folder_outside_takes_its_images_along() {
    let (dir, mut store) = temp("moved-images");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n![shot](.attachments/shot.png)\n");
    fs::create_dir_all(dir.path.join(".attachments")).unwrap();
    write(&dir.path.join(".attachments/shot.png"), "png");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    let folder = dir.path.join("Work");
    fs::create_dir_all(&folder).unwrap();
    let moved = folder.join("Plan.md");
    fs::rename(&path, &moved).unwrap();
    app.check();
    assert_eq!(app.current_path(), Some(moved.as_path()));
    assert!(folder.join(".attachments/shot.png").exists());
}

#[test]
fn an_image_pasted_but_not_saved_goes_along_with_a_note_moved_outside() {
    let (dir, mut store) = temp("moved-unsaved-image");
    let path = store.create("", Some("Plan"));
    write(&path, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(store, options());
    app.start();
    app.open_note(&path, false, false);
    fs::create_dir_all(dir.path.join(".attachments")).unwrap();
    write(&dir.path.join(".attachments/new.png"), "png");
    app.editor
        .change(|doc| doc.text.set_line(2, "![new](.attachments/new.png)"));
    let folder = dir.path.join("Work");
    fs::create_dir_all(&folder).unwrap();
    let moved = folder.join("Plan.md");
    fs::rename(&path, &moved).unwrap();
    app.save();
    assert_eq!(app.current_path(), Some(moved.as_path()));
    assert!(folder.join(".attachments/new.png").exists());
}

// UX 2.0 board 8: an item's plan beside the note.

const PLAN_A_B: &str = "# Plan - Condensed sidebar

Item: \"a condensed sidebar\"

## Today

- The sidebar is 34 columns wide, each tab on two lines.

## Decision needed

- [ ] A (recommended): focus widens it
  ```mock
  ● 2 stable note IDs
  ```
- [ ] B: stays condensed

## Steps (A)

1. A sidebar_condensed setting
2. Tests
";

fn with_plan(name: &str, cols: u16) -> (Dir, App, PathBuf, PathBuf) {
    let (dir, mut app) = started_ux2(name);
    app.cols = cols;
    let plan = app
        .store
        .create("Switchboard Plans", Some("Plan - Condensed sidebar"));
    write(&plan, PLAN_A_B);
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] a condensed sidebar\n  Plan: [[Plan - Condensed sidebar]] (needs a choice: A, B)\n- [ ] bare\n",
    );
    app.open_note(&path, false, false);
    (dir, app, path, plan)
}

fn alt(c: char) -> Key {
    Key::new(
        KeyCode::Char(c),
        Mods {
            ctrl: false,
            shift: false,
            alt: true,
        },
    )
}

#[test]
fn the_plan_beside_the_note_shows_its_choice_as_cards_and_a_letter_chooses() {
    let (_dir, mut app, path, plan) = with_plan("beside", 160);
    keys(&mut app, "4G<CR>");
    let shown = screen(&mut app);
    let text = shown.join("\n");
    // The list put away, the note narrowed beside the plan.
    let (header_x, _) = find(&shown, "Plan Condensed sidebar");
    assert!(header_x > 40 && header_x < 80, "{text}");
    assert!(text.contains("Switchboard Plans"), "the folder: {text}");
    assert!(text.contains("TODAY"), "{text}");
    assert!(text.contains("DECISION NEEDED"), "{text}");
    let (a_x, a_y) = find(&shown, "A · focus widens it");
    let (b_x, b_y) = find(&shown, "B · stays condensed");
    assert_eq!(a_y, b_y, "the cards side by side: {text}");
    assert!(b_x > a_x);
    assert!(text.contains("recommended"));
    assert!(text.contains("● 2 stable note IDs"), "the mock: {text}");
    assert!(
        text.contains("Choose A") && text.contains("Choose B"),
        "{text}"
    );
    assert!(text.contains("STEPS (A)"));
    assert!(text.contains("1. A sidebar_condensed setting"));
    let footer = shown.last().unwrap();
    assert!(footer.contains("C-w l plan"), "{footer}");
    assert!(!footer.contains("a/b choose"), "{footer}");

    // C-w l gives the plan the keys, C-w h the note back.
    keys(&mut app, "<C-w>l");
    assert!(app.beside.as_ref().unwrap().focused);
    keys(&mut app, "<C-w>h");
    assert!(!app.beside.as_ref().unwrap().focused);
    assert_eq!(app.focus, Focus::Note);

    // b in the plan chooses B: written into the plan, and the item says so.
    keys(&mut app, "<C-w>lb");
    let written = fs::read_to_string(&plan).unwrap();
    assert!(written.contains("## Decision\n"), "{written}");
    assert!(written.contains("- [x] B: stays condensed"), "{written}");
    assert!(written.contains("## Steps (B)"), "{written}");
    assert_eq!(
        lines(&app)[3],
        "  Plan: [[Plan - Condensed sidebar]] (chose B)"
    );
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("(chose B)"), "{saved}");
    // u in the note doesn't take the choice back from the item alone.
    keys(&mut app, "<C-w>hu");
    assert_eq!(
        lines(&app)[3],
        "  Plan: [[Plan - Condensed sidebar]] (chose B)"
    );
    keys(&mut app, "<C-w>l");
    assert!(app.beside.as_ref().unwrap().plan.choice().is_none());

    // Esc closes it, the note widens again.
    keys(&mut app, "<Esc>");
    assert!(app.beside.is_none());
    assert!(
        !screen(&mut app)
            .join("\n")
            .contains("Plan Condensed sidebar")
    );
}

/// The screen drawn, and where the terminal's cursor goes.
fn drawn(app: &mut App) -> (Vec<String>, Option<(u16, u16)>) {
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    let cursor = app.draw(&mut buf);
    let shown = (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect::<String>()
                .trim_end()
                .to_string()
        })
        .collect();
    (shown, cursor)
}

#[test]
fn the_note_is_greyed_while_the_plan_has_the_keys_and_the_plan_while_the_note_does() {
    let (_dir, mut app, _path, _plan) = with_plan("beside greyed", 160);
    app.rows = 14;
    keys(&mut app, "4G<CR>");
    let surface = app.look.theme.panel;
    // The background at the foot of the note and of the plan.
    let feet = |app: &mut App| {
        let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
        app.draw(&mut buf);
        let (note, plan) = (app.geo.note, app.geo.plan);
        (
            buf[(note.right() - 2, note.bottom() - 1)].bg,
            buf[(plan.right() - 2, plan.bottom() - 1)].bg,
        )
    };
    let (note, plan) = feet(&mut app);
    assert_ne!(note, surface, "the note has the keys");
    assert_eq!(plan, surface);
    keys(&mut app, "<C-w>l");
    let (note, plan) = feet(&mut app);
    assert_eq!(note, surface);
    assert_ne!(plan, surface, "the plan has the keys");
}

#[test]
fn the_plan_with_the_keys_shows_the_cursor_and_its_keys() {
    let (_dir, mut app, _path, plan) = with_plan("beside cursor", 160);
    app.rows = 14;
    keys(&mut app, "4G<CR>");
    let (shown, in_note) = drawn(&mut app);
    let footer = shown.last().unwrap();
    assert!(footer.contains("C-w l plan"), "{footer}");
    assert!(!footer.contains("Esc close"), "{footer}");
    let in_note = in_note.unwrap();
    assert!(in_note.0 < app.geo.plan.x, "{in_note:?}");

    // C-w puts the cursor on the plan's first line, j a line down.
    keys(&mut app, "<C-w>l");
    let (shown, cursor) = drawn(&mut app);
    let x = app.geo.plan.x + 1;
    assert_eq!(cursor, Some((x, 1)));
    let footer = shown.last().unwrap();
    assert!(!footer.contains("a/b choose"), "{footer}");
    assert!(footer.contains("Enter start the item"), "{footer}");
    assert!(footer.contains("Esc close"), "{footer}");
    assert!(footer.contains("C-w h note"), "{footer}");
    keys(&mut app, "j");
    assert_eq!(drawn(&mut app).1, Some((x, 2)));

    // At the plan's bottom it scrolls, keeping the cursor in sight.
    let body = app.geo.plan.height - 1;
    for _ in 0..body + 2 {
        keys(&mut app, "j");
    }
    let (_, cursor) = drawn(&mut app);
    assert!(app.beside.as_ref().unwrap().scroll > 0);
    assert_eq!(cursor.map(|c| c.1), Some(body));

    // C-w back puts the cursor where it was in the note.
    keys(&mut app, "<C-w>h");
    assert_eq!(drawn(&mut app).1, Some(in_note));

    // Enter on option A's card chooses A.
    keys(&mut app, "<C-w>lg");
    for _ in 0..40 {
        if app.beside.as_ref().unwrap().stop == Some(beside::Stop::Card('A')) {
            break;
        }
        keys(&mut app, "j");
        drawn(&mut app);
    }
    let footer = drawn(&mut app).0.last().unwrap().clone();
    assert!(footer.contains("Enter choose A"), "{footer}");
    keys(&mut app, "<CR>");
    let written = fs::read_to_string(&plan).unwrap();
    assert!(written.contains("- [x] A (recommended)"), "{written}");
}

#[test]
fn the_card_lit_follows_the_cursor_after_the_wheel_and_the_note() {
    let (_dir, mut app, _path, plan) = with_plan("beside cursor lit", 160);
    let steps: String = (3..20).map(|n| format!("{n}. Step {n}\n")).collect();
    write(&plan, &format!("{PLAN_A_B}{steps}"));
    app.rows = 14;
    keys(&mut app, "4G<CR><C-w>l<Tab>");
    drawn(&mut app);
    assert_eq!(
        app.beside.as_ref().unwrap().stop,
        Some(beside::Stop::Card('A'))
    );

    // Tab past the last card and BackTab before the first light none.
    keys(&mut app, "<Tab><Tab>");
    drawn(&mut app);
    assert_eq!(app.beside.as_ref().unwrap().stop, None);
    keys(&mut app, "<Tab>");
    for want in [None, Some('B')] {
        app.key(Key::new(KeyCode::BackTab, Mods::NONE));
        drawn(&mut app);
        assert_eq!(
            app.beside.as_ref().unwrap().stop,
            want.map(beside::Stop::Card)
        );
    }
    // B, beside A on the same rows, is still lit after the note and back.
    keys(&mut app, "<C-w>h");
    assert!(!screen(&mut app).join("\n").contains("Enter choose"));
    keys(&mut app, "<C-w>l");
    drawn(&mut app);
    assert_eq!(
        app.beside.as_ref().unwrap().stop,
        Some(beside::Stop::Card('B'))
    );
    // And after a click in the note.
    click(&mut app, 5, 1);
    assert!(!app.beside.as_ref().unwrap().focused);
    keys(&mut app, "<C-w>l");
    drawn(&mut app);
    assert_eq!(
        app.beside.as_ref().unwrap().stop,
        Some(beside::Stop::Card('B'))
    );
    keys(&mut app, "<Tab><Tab>");

    // The note and back: the cursor still on the card, it is lit again.
    keys(&mut app, "<C-w>h<C-w>l");
    drawn(&mut app);
    assert_eq!(
        app.beside.as_ref().unwrap().stop,
        Some(beside::Stop::Card('A'))
    );

    // The wheel takes the cursor past the cards: nothing lit, Enter
    // chooses nothing.
    let (x, y) = (app.geo.plan.x + 3, 3);
    for _ in 0..10 {
        app.mouse(Mouse {
            kind: MouseKind::Press,
            button: MouseButton::WheelDown,
            col: x,
            row: y,
            mods: Mods::NONE,
        });
        drawn(&mut app);
    }
    assert_eq!(app.beside.as_ref().unwrap().stop, None);
    let before = fs::read_to_string(&plan).unwrap();
    assert!(!before.contains("[x]"));
}

#[test]
fn a_click_on_a_plan_line_puts_the_cursor_there() {
    let (_dir, mut app, _path, _plan) = with_plan("beside cursor click", 160);
    keys(&mut app, "4G<CR>");
    drawn(&mut app);
    assert!(!app.beside.as_ref().unwrap().focused);
    let x = app.geo.plan.x + 3;
    click(&mut app, x, 4);
    assert!(app.beside.as_ref().unwrap().focused);
    assert_eq!(drawn(&mut app).1, Some((app.geo.plan.x + 1, 4)));
}

#[test]
fn a_click_on_a_card_chooses_it_and_e_opens_the_plan_note() {
    let (_dir, mut app, _path, plan) = with_plan("beside click", 160);
    keys(&mut app, "4G<CR>");
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "Choose A");
    click(&mut app, x + 1, y);
    let written = fs::read_to_string(&plan).unwrap();
    assert!(written.contains("- [x] A (recommended)"), "{written}");
    assert!(app.beside.as_ref().unwrap().focused);
    keys(&mut app, "e");
    assert!(app.beside.is_none());
    assert_eq!(name(&app), "Plan - Condensed sidebar.md");
}

#[test]
fn a_paste_with_the_plan_focused_and_going_to_the_list_leave_the_note_alone() {
    let (_dir, mut app, _path, _plan) = with_plan("beside paste", 160);
    keys(&mut app, "4G<CR><C-w>l");
    app.paste("pasted");
    assert!(!lines(&app).iter().any(|l| l.contains("pasted")));
    assert_eq!(
        app.request(socket::Request::Run("Go to the sidebar".into())),
        "ok"
    );
    assert!(app.beside.is_none());
    assert!(app.list_shown);
    assert_eq!(app.focus, Focus::List);

    // Ctrl-B from the plan shows the list too.
    keys(&mut app, "<Tab>");
    app.focus = Focus::Note;
    keys(&mut app, "4G<CR><C-w>l<C-b>");
    assert!(app.beside.is_none());
    assert!(app.list_shown);

    // Narrow, a click outside the plan leaves it the keys while it hides
    // the note.
    app.cols = 90;
    app.focus = Focus::Note;
    keys(&mut app, "4G<CR>");
    let _ = screen(&mut app);
    let y = app.rows - 2;
    click(&mut app, 2, y);
    keys(&mut app, "<Esc>");
    assert!(app.beside.as_ref().is_some_and(|b| b.focused));
}

#[test]
fn choosing_turns_the_item_s_needs_a_choice_past_a_blank_line() {
    let (_dir, mut app) = started_ux2("beside blank");
    app.cols = 160;
    let plan = app.store.create("", Some("Plan - Condensed sidebar"));
    write(&plan, PLAN_A_B);
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] a condensed sidebar\n\n  Plan: [[Plan - Condensed sidebar]] (needs a choice: A, B)\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "5G<CR><C-w>la");
    assert_eq!(
        lines(&app)[4],
        "  Plan: [[Plan - Condensed sidebar]] (chose A)"
    );
}

#[test]
fn choosing_on_the_item_s_own_line_keeps_the_panel_on_the_item() {
    let (_dir, mut app) = started_ux2("beside first line");
    app.cols = 160;
    let plan = app.store.create("", Some("Plan - Condensed sidebar"));
    write(&plan, PLAN_A_B);
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] a condensed sidebar (needs a choice: A, B)\n  Plan: [[Plan - Condensed sidebar]]\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "4G<CR><C-w>la");
    assert_eq!(lines(&app)[2], "- [ ] a condensed sidebar (chose A)");
    keys(&mut app, "<C-w>h3G");
    assert!(app.beside_is_of_cursor());
}

#[test]
fn only_the_first_open_decision_is_drawn_as_cards() {
    let (_dir, mut app, _path, plan) = with_plan("beside two decisions", 160);
    let two = PLAN_A_B.replace(
        "## Steps (A)",
        "## Decision needed\n\n- [ ] A: later one\n- [ ] B: later two\n\n## Steps (A)",
    );
    write(&plan, &two);
    keys(&mut app, "4G<CR>");
    let text = screen(&mut app).join("\n");
    assert!(text.contains("B · stays condensed"), "{text}");
    assert!(!text.contains("B · later two"), "{text}");
    assert!(text.contains("- B: later two"), "{text}");
}

#[test]
fn choosing_passes_a_sub_item_s_own_choice() {
    let (_dir, mut app) = started_ux2("beside sub item");
    app.cols = 160;
    let plan = app.store.create("", Some("Plan - Condensed sidebar"));
    write(&plan, PLAN_A_B);
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] a condensed sidebar\n  - [ ] a part (needs a choice: X)\n  Plan: [[Plan - Condensed sidebar]] (needs a choice: A, B)\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "5G<CR><C-w>la");
    assert_eq!(lines(&app)[3], "  - [ ] a part (needs a choice: X)");
    assert_eq!(
        lines(&app)[4],
        "  Plan: [[Plan - Condensed sidebar]] (chose A)"
    );
}

#[test]
fn a_plan_changed_on_disk_is_shown_again_before_a_choice() {
    let (_dir, mut app, _path, plan) = with_plan("beside changed", 160);
    keys(&mut app, "4G<CR>");
    write(
        &plan,
        &PLAN_A_B.replace("focus widens it", "focus grows it"),
    );
    keys(&mut app, "<C-w>la");
    let written = fs::read_to_string(&plan).unwrap();
    assert!(
        written.contains("- [ ] A (recommended): focus grows it"),
        "{written}"
    );
    assert_eq!(
        app.message.as_ref().map(|m| m.0.as_str()),
        Some("notes: the plan changed; look again and choose")
    );
    assert!(screen(&mut app).join("\n").contains("focus grows it"));
    keys(&mut app, "a");
    assert!(fs::read_to_string(&plan).unwrap().contains("- [x] A"));
}

#[test]
fn a_later_open_decision_without_options_keeps_the_item_needing_a_choice() {
    let (_dir, mut app, _path, plan) = with_plan("beside open question", 160);
    write(
        &plan,
        &PLAN_A_B.replace(
            "## Steps (A)",
            "## Decision needed\n\nWhich color?\n\n## Steps (A)",
        ),
    );
    keys(&mut app, "4G<CR><C-w>la");
    assert!(
        lines(&app)[3].contains("(needs a choice: A, B)"),
        "{:?}",
        lines(&app)
    );
}

#[test]
fn an_item_reworded_while_its_plan_shows_is_still_the_one_chosen_for() {
    let (_dir, mut app, _path, _plan) = with_plan("beside reworded", 160);
    keys(&mut app, "4G<CR>3GA now<Esc><C-w>lb");
    assert_eq!(lines(&app)[2], "- [ ] a condensed sidebar now");
    assert_eq!(
        lines(&app)[3],
        "  Plan: [[Plan - Condensed sidebar]] (chose B)"
    );
    assert_eq!(app.beside.as_ref().unwrap().item, "a condensed sidebar now");
    keys(&mut app, "<C-w>h3G");
    assert!(app.beside_is_of_cursor());
}

#[test]
fn a_palette_command_that_starts_typing_gives_the_note_the_keys() {
    let (_dir, mut app, _path, plan) = with_plan("beside palette typing", 160);
    keys(&mut app, "4G<CR><C-w>l");
    let bold = palette::commands(false, true, true)
        .into_iter()
        .find(|c| c.name.ends_with("Bold"))
        .unwrap()
        .name;
    assert_eq!(app.request(socket::Request::Run(bold)), "ok");
    keys(&mut app, "b");
    assert!(!app.beside.as_ref().unwrap().focused);
    assert!(!fs::read_to_string(&plan).unwrap().contains("[x]"));

    // An indent leaves Normal mode as it was, and still gives the note the
    // keys.
    keys(&mut app, "<Esc><C-w>l");
    assert!(app.beside.as_ref().unwrap().focused);
    let indent = palette::commands(false, true, true)
        .into_iter()
        .find(|c| c.act == PaletteAct::Indent)
        .unwrap()
        .name;
    assert_eq!(app.request(socket::Request::Run(indent)), "ok");
    assert!(!app.beside.as_ref().unwrap().focused);
}

#[test]
fn an_item_linking_another_plan_since_is_not_chosen_for() {
    let (_dir, mut app, _path, plan) = with_plan("beside relinked", 160);
    keys(&mut app, "4G<CR>4G");
    keys(&mut app, "0f-lct]New<Esc>");
    assert!(
        lines(&app)[3].contains("[[Plan -New]]"),
        "{:?}",
        lines(&app)
    );
    keys(&mut app, "<C-w>la");
    assert!(!fs::read_to_string(&plan).unwrap().contains("[x]"));
    assert!(lines(&app)[3].contains("needs a choice"));
}

#[test]
fn a_plan_whose_title_differs_from_its_file_name_is_chosen_for() {
    let (_dir, mut app) = started_ux2("beside colon title");
    app.cols = 160;
    let plan = app.store.create("", Some("Plan - UI: layout"));
    write(
        &plan,
        &PLAN_A_B.replace("# Plan - Condensed sidebar", "# Plan - UI: layout"),
    );
    let path = app.store.create("", Some("Design"));
    write(
        &path,
        "# Design\n\n- [ ] the layout\n  Plan: [[Plan - UI: layout]] (needs a choice: A, B)\n",
    );
    app.open_note(&path, false, false);
    keys(&mut app, "4G<CR><C-w>la");
    assert_eq!(lines(&app)[3], "  Plan: [[Plan - UI: layout]] (chose A)");
}

#[test]
fn tab_to_a_card_below_the_view_scrolls_it_into_view() {
    let (_dir, mut app, _path, plan) = with_plan("beside long today", 160);
    let long = PLAN_A_B.replace(
        "- The sidebar is 34 columns wide, each tab on two lines.",
        &"- A line of today.\n".repeat(40),
    );
    write(&plan, &long);
    keys(&mut app, "4G<CR>");
    assert!(!screen(&mut app).join("\n").contains("Choose A"));
    keys(&mut app, "<C-w>l<Tab>");
    assert!(screen(&mut app).join("\n").contains("Choose A"));
}

#[test]
fn the_panel_goes_once_its_item_no_longer_links_the_plan_and_the_chip_only_shows_it() {
    let (_dir, mut app, _path, _plan) = with_plan("beside unlinked", 160);
    keys(&mut app, "4G<CR>3G");
    // The palette's "Open this item's plan" again keeps the plan shown.
    assert!(app.open_plan());
    assert!(app.beside.is_some());
    assert!(app.message.is_none());
    keys(&mut app, "4Gdd");
    app.save_due = Some(Instant::now());
    app.tick();
    assert!(app.beside.is_none());
}

#[test]
fn search_results_put_the_plan_away() {
    let (_dir, mut app, _path, _plan) = with_plan("beside search", 160);
    keys(&mut app, "4G<CR>");
    app.show(None, None, Some("sidebar".into()));
    assert!(app.beside.is_none());
    assert!(app.list_shown);
}

#[test]
fn opening_another_note_puts_the_plan_away() {
    let (_dir, mut app, _path, _plan) = with_plan("beside other note", 160);
    keys(&mut app, "4G<CR>");
    assert!(app.beside.is_some());
    let other = app.store.create("", Some("Other"));
    write(&other, "# Other\n");
    app.open_note(&other, false, false);
    assert!(app.beside.is_none());
}

#[test]
fn m_e_opens_the_plan_of_the_item_under_the_cursor_and_closes_it() {
    let (_dir, mut app, _path, _plan) = with_plan("beside m-e", 160);
    keys(&mut app, "3G");
    app.key(alt('e'));
    assert!(app.beside.is_some());
    app.key(alt('e'));
    assert!(app.beside.is_none());
    keys(&mut app, "1G");
    app.key(alt('e'));
    assert!(app.beside.is_none());
    assert_eq!(
        app.message.as_ref().map(|m| m.0.as_str()),
        Some("No plan for this line")
    );
}

#[test]
fn the_plan_fills_the_width_in_its_share_and_drops_its_extras_when_narrow() {
    // Wide: the plan's text stops at 100 columns, the folder at the right.
    let (_dir, mut app, _path, _plan) = with_plan("beside wide", 240);
    keys(&mut app, "4G<CR>");
    let shown = screen(&mut app);
    let plan = app.geo.plan;
    let note = app.geo.note;
    assert_eq!(note.width + 1 + plan.width, 240);
    assert!(plan.width > note.width, "{note:?} {plan:?}");
    let header = &shown[0];
    assert!(header.trim_end().ends_with("Switchboard Plans"), "{header}");

    // The board's width: everything.
    let (_dir, mut app, _path, _plan) = with_plan("beside board", 125);
    keys(&mut app, "4G<CR>");
    let text = screen(&mut app).join("\n");
    assert!(
        text.contains("Switchboard Plans") && text.contains("● 2 stable"),
        "{text}"
    );

    // Narrower: the folder goes, then the mocks, then the cards stack.
    let (_dir, mut app, _path, _plan) = with_plan("beside 120", 120);
    keys(&mut app, "4G<CR>");
    let text = screen(&mut app).join("\n");
    assert!(!text.contains("Switchboard Plans"), "{text}");
    assert!(text.contains("● 2 stable"), "{text}");
    let (_dir, mut app, _path, _plan) = with_plan("beside 115", 115);
    keys(&mut app, "4G<CR>");
    let text = screen(&mut app).join("\n");
    assert!(!text.contains("● 2 stable"), "{text}");
    let shown = screen(&mut app);
    let (_, a_y) = find(&shown, "A · focus");
    let (_, b_y) = find(&shown, "B · stays");
    assert_eq!(a_y, b_y);
    let (_dir, mut app, _path, _plan) = with_plan("beside 108", 108);
    keys(&mut app, "4G<CR>");
    let shown = screen(&mut app);
    let (_, a_y) = find(&shown, "A · focus");
    let (_, b_y) = find(&shown, "B · stays");
    assert!(b_y > a_y, "stacked:\n{}", shown.join("\n"));
    assert!(app.geo.note.width > 0, "the note still beside the plan");

    // Under the narrowest, the plan takes the width, and the command shows
    // the note in its place; Esc there gives the plan back.
    let (_dir, mut app, path, _plan) = with_plan("beside 90", 90);
    keys(&mut app, "4G<CR>");
    let _ = screen(&mut app);
    assert_eq!(app.geo.note.width, 0);
    // The plan has the keys, so typing doesn't reach the hidden note.
    assert!(app.beside.as_ref().unwrap().focused);
    keys(&mut app, "ix");
    assert_eq!(lines(&app)[2], "- [ ] a condensed sidebar");
    let _ = path;
    assert_eq!(app.geo.plan.width, 90);
    assert_eq!(
        app.request(socket::Request::Run("Show the note beside its plan".into())),
        "ok"
    );
    let _ = screen(&mut app);
    assert_eq!(app.geo.plan.width, 0);
    assert_eq!(app.geo.note.width, 90);
    keys(&mut app, "<Esc>");
    let _ = screen(&mut app);
    assert_eq!(app.geo.plan.width, 90);
}

#[test]
fn enter_on_the_item_whose_plan_shows_starts_work_on_it() {
    let (_dir, mut app, _path, _plan) = with_plan("beside start", 160);
    keys(&mut app, "4G<CR>");
    assert!(app.beside.is_some());
    keys(&mut app, "<CR>");
    // Outside the workspace, gw's press says it needs it.
    assert_eq!(
        app.message.as_ref().map(|m| m.0.as_str()),
        Some("notes: that needs Switchboard's workspace")
    );
}

#[test]
fn an_item_put_next_in_autopilot_s_queue_has_its_chip() {
    let (_dir, mut app, path, _plan) = with_plan("next chip", 160);
    let id = app.open.as_ref().and_then(|o| o.id.clone()).unwrap();
    app.handle(Msg::Next(
        [note_item_key(&id, "bare")].into_iter().collect(),
    ));
    let text = screen(&mut app).join("\n");
    assert!(text.contains("next in the queue"), "{text}");
    let _ = path;
}

#[test]
fn opening_a_note_again_checks_its_id_only_once_its_file_or_the_notes_changed() {
    let (dir, mut app) = started("ids_checked");
    let first = dir.path.join("First.md");
    let other = dir.path.join("Other.md");
    write(&first, "# First\n\nwords\n");
    write(&other, "# Other\n\nmore\n");
    set_mtime(&first, crate::util::now_secs() - 100);
    set_mtime(&other, crate::util::now_secs() - 200);
    app.open_note(&first, false, false);
    app.open_note(&other, false, false);
    let first_id = id::of_file(&first).unwrap();
    // Other given First's ID while keeping an older time: a check would now
    // take First for the copy, so First keeping its ID on opening again
    // shows it wasn't read for.
    let other_text = fs::read_to_string(&other).unwrap();
    write(&other, &id::with(&id::body_of(&other_text), &first_id));
    set_mtime(&other, crate::util::now_secs() - 200);
    app.open_note(&first, false, false);
    assert_eq!(
        id::of_file(&first).as_deref(),
        Some(first_id.as_str()),
        "a note opened again with its file unchanged isn't checked again"
    );
    // Its file changed: checked again, and the copy it now reads as gets an
    // ID of its own.
    app.editor.doc.text = Text::from_file("# First\n\nwords, typed\n");
    app.save();
    set_mtime(&first, crate::util::now_secs() - 100);
    app.open_note(&other, false, false);
    app.open_note(&first, false, false);
    assert_ne!(
        id::of_file(&first).as_deref(),
        Some(first_id.as_str()),
        "a note whose file changed is checked again"
    );
}

#[test]
fn a_copy_made_while_notes_runs_is_found_when_the_original_opens_again() {
    let (dir, mut app) = started("ids_copy");
    let old = dir.path.join("Old.md");
    let other = dir.path.join("Other.md");
    write(&old, "# Old\n\nwords\n");
    write(&other, "# Other\n\nmore\n");
    set_mtime(&old, crate::util::now_secs() - 100);
    app.open_note(&old, false, false);
    app.open_note(&other, false, false);
    let old_id = id::of_file(&old).unwrap();
    // A copy holding the same ID, made earlier than the original's last
    // change, so the original is the one that reads as the copy.
    let copy = dir.path.join("Old copy.md");
    fs::copy(&old, &copy).unwrap();
    set_mtime(&copy, crate::util::now_secs() - 300);
    app.open_note(&old, false, false);
    assert_ne!(
        id::of_file(&old).as_deref(),
        Some(old_id.as_str()),
        "a new note file has every note checked again"
    );
    assert_eq!(id::of_file(&copy).as_deref(), Some(old_id.as_str()));
}

const DIAGRAM_NOTE: &str = "# Flow\n\nintro\n```mermaid\ngraph LR\n  subgraph ci[\"CI\"]\n    build[Build] --> deploy[Deploy]\n  end\n```\nafter\n```mermaid\npie\n  \"a\" : 1\n```\n";

fn diagram_app(name: &str, opts: Options) -> (Dir, App) {
    let (dir, mut store) = temp(name);
    let path = store.create("", Some("Flow"));
    write(&path, DIAGRAM_NOTE);
    let (mut app, _) = app_in(store, opts);
    app.start();
    keys(&mut app, "gg");
    (dir, app)
}

fn shows(shown: &[String], text: &str) -> bool {
    shown.iter().any(|l| l.contains(text))
}

#[test]
fn a_mermaid_block_draws_as_its_diagram_and_as_its_code_with_the_cursor_in_it() {
    let (_dir, mut app) = diagram_app("mermaid", options());
    let shown = screen(&mut app);
    let all = shown.join("\n");
    assert!(shows(&shown, "│ Build ├"), "drawn:\n{all}");
    assert!(shows(&shown, " CI "), "its subgraph's title:\n{all}");
    assert!(!shows(&shown, "graph LR"), "its code hidden:\n{all}");
    assert!(shows(&shown, "after"), "{all}");
    // A pie isn't drawn: its code shows, with a line saying so.
    assert!(shows(&shown, "pie"), "{all}");
    assert!(
        shows(
            &shown,
            "Pie charts aren't drawn in Notes; its code shows instead."
        ),
        "{all}"
    );
    // Down onto the block: its first line, and its code.
    keys(&mut app, "jjj");
    assert_eq!(app.editor.doc.row, 3);
    let shown = screen(&mut app);
    assert!(shows(&shown, "graph LR"), "{}", shown.join("\n"));
    assert!(!shows(&shown, "│ Build ├"), "{}", shown.join("\n"));
    // Out of it again: the diagram.
    keys(&mut app, "G");
    let shown = screen(&mut app);
    assert!(shows(&shown, "│ Build ├"), "{}", shown.join("\n"));
    assert!(!shows(&shown, "graph LR"), "{}", shown.join("\n"));
}

#[test]
fn show_diagram_code_pins_a_notes_blocks_to_their_code_until_show_diagrams() {
    let (_dir, mut app) = diagram_app("mermaid-code", options());
    keys(&mut app, "<C-p>/diagram code<CR>");
    let shown = screen(&mut app);
    assert!(shows(&shown, "graph LR"), "{}", shown.join("\n"));
    assert!(!shows(&shown, "│ Build ├"), "{}", shown.join("\n"));
    let named = |app: &App| {
        app.palette_commands(false)
            .into_iter()
            .find(|c| c.act == PaletteAct::Format(FormatAct::DiagramCode))
            .map(|c| c.name)
    };
    assert_eq!(named(&app).as_deref(), Some("Diagram: Show diagrams"));
    keys(&mut app, "<C-p>/show diagrams");
    let shown = screen(&mut app);
    assert!(shows(&shown, "Show diagrams"), "{}", shown.join("\n"));
    keys(&mut app, "<CR>");
    let shown = screen(&mut app);
    assert!(shows(&shown, "│ Build ├"), "{}", shown.join("\n"));
    assert_eq!(named(&app).as_deref(), Some("Diagram: Show diagram code"));
}

#[test]
fn with_the_setting_off_every_block_shows_its_code() {
    let (_dir, mut app) = diagram_app(
        "mermaid-off",
        Options {
            mermaid: false,
            ..options()
        },
    );
    let shown = screen(&mut app);
    assert!(shows(&shown, "graph LR"), "{}", shown.join("\n"));
    assert!(!shows(&shown, "│ Build ├"), "{}", shown.join("\n"));
    assert!(!shows(&shown, "aren't drawn"), "{}", shown.join("\n"));
}

#[test]
fn a_diagram_wider_than_the_note_is_cut_at_its_edge() {
    let (_dir, mut store) = temp("mermaid-wide");
    let path = store.create("", Some("Wide"));
    let children: Vec<String> = (0..12)
        .map(|i| format!("d --> s{i}[service-{i}]"))
        .collect();
    write(
        &path,
        &format!(
            "# Wide\n\n```mermaid\ngraph TD\n{}\n```\n",
            children.join("\n")
        ),
    );
    let (mut app, _) = app_in(store, options());
    app.start();
    keys(&mut app, "gg");
    let shown = screen(&mut app);
    let all = shown.join("\n");
    assert!(shows(&shown, "service-0"), "{all}");
    assert!(shown.iter().any(|l| l.ends_with('›')), "{all}");
    assert!(shows(&shown, crate::diagram::CUT), "{all}");
}

#[test]
fn a_diagram_taller_than_the_note_is_cut_to_fit_it() {
    let (_dir, mut store) = temp("mermaid-tall");
    let path = store.create("", Some("Tall"));
    let chain: Vec<String> = (0..12)
        .map(|i| format!("n{i}[Step {i}] --> n{}[Step {}]", i + 1, i + 1))
        .collect();
    write(
        &path,
        &format!(
            "# Tall\n\n```mermaid\ngraph TD\n{}\n```\n",
            chain.join("\n")
        ),
    );
    let (mut app, _) = app_in(store, options());
    app.start();
    keys(&mut app, "gg");
    let shown = screen(&mut app);
    let all = shown.join("\n");
    assert!(shows(&shown, "Step 0"), "{all}");
    // The wheel scrolls it to the top, its rows and the line under it on
    // the screen, still drawn.
    for _ in 0..4 {
        let r = app.geo.note;
        app.mouse(Mouse {
            kind: MouseKind::Press,
            button: MouseButton::WheelDown,
            col: r.x + 2,
            row: r.y + 2,
            mods: Mods::NONE,
        });
        screen(&mut app);
    }
    let shown = screen(&mut app);
    assert!(shows(&shown, "Step 0"), "{}", shown.join("\n"));
    assert!(shows(&shown, crate::diagram::CUT), "{}", shown.join("\n"));
}

#[test]
fn a_note_open_in_another_app_is_read_only_until_that_app_lets_it_go() {
    let (_dir, store) = temp("held");
    let plan = store.absolute("Plan.md");
    write(&plan, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    assert!(app.held.is_none(), "a note no app has is this one's");
    let key = id::in_text(&fs::read_to_string(&plan).unwrap()).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    assert!(lock.exists(), "its lock taken as it opened");

    // Another app, a live process, has it: shown read-only.
    let other = unsafe { libc::getppid() } as i64;
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.open_note(&plan, false, false);
    assert!(app.held.is_some());
    let before = fs::read_to_string(&plan).unwrap();
    keys(&mut app, "Ohello<Esc>");
    app.save();
    assert_eq!(
        lines(&app),
        ["# Plan", "", "- [ ] one"],
        "the edit taken back"
    );
    assert_eq!(
        fs::read_to_string(&plan).unwrap(),
        before,
        "nothing written"
    );
    app.message = None;
    assert!(
        screen(&mut app)
            .iter()
            .any(|l| l.contains("Open in another Notes, read-only here")),
        "{:#?}",
        screen(&mut app)
    );

    // That app gone, the note is this one's to edit.
    fs::write(&lock, "999999999 %9").unwrap();
    app.held.as_mut().unwrap().checked = Instant::now() - HELD_CHECK;
    assert!(app.tick());
    assert!(app.held.is_none());
    keys(&mut app, "Gohello<Esc>");
    app.save();
    assert!(fs::read_to_string(&plan).unwrap().contains("hello"));
    app.stop();
    assert!(!lock.exists(), "let go as it ends");
}

#[test]
fn an_edit_tried_on_a_held_note_opens_a_box_naming_its_tab() {
    let (_dir, store) = temp("held-box");
    let plan = store.absolute("Plan.md");
    write(&plan, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            server: true,
            ..options()
        },
    );
    app.start();
    let key = id::in_text(&fs::read_to_string(&plan).unwrap()).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    let other = unsafe { libc::getppid() } as i64;
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.open_note(&plan, false, false);
    app.held.as_mut().unwrap().tab = Some(4);

    keys(&mut app, "i");
    assert_eq!(app.top_kind(), Some("menu"));
    assert!(matches!(app.editor.mode, Mode::Normal));
    let shown = screen(&mut app);
    assert!(
        shown
            .iter()
            .any(|l| l.contains("Plan is open in tab 4, so it's read-only here")),
        "{shown:#?}"
    );
    for option in ["Go to tab 4", "Take it over", "Keep reading"] {
        assert!(shown.iter().any(|l| l.contains(option)), "{option}");
    }

    // Typing on goes to the box, never a second one.
    keys(&mut app, "abc");
    assert_eq!(app.layers.len(), 1);
    keys(&mut app, "<Esc>");
    assert_eq!(app.top_kind(), None, "Esc keeps reading");
    assert!(matches!(app.editor.mode, Mode::Normal));
    assert_eq!(lines(&app), ["# Plan", "", "- [ ] one"]);

    // Answered, the next try asks again.
    keys(&mut app, "dd");
    assert_eq!(app.top_kind(), Some("menu"));
    assert_eq!(lines(&app), ["# Plan", "", "- [ ] one"]);

    // Let go meanwhile, the box closes and the note is this app's.
    fs::write(&lock, "999999999 %9").unwrap();
    app.held.as_mut().unwrap().checked = Instant::now() - HELD_CHECK;
    assert!(app.tick());
    assert!(app.held.is_none());
    assert_eq!(app.top_kind(), None);
}

#[test]
fn a_note_given_to_another_app_is_saved_let_go_and_shown_read_only() {
    let (_dir, store) = temp("held-give");
    let plan = store.absolute("Plan.md");
    write(&plan, "# Plan\n\none\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    let key = id::in_text(&fs::read_to_string(&plan).unwrap()).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    keys(&mut app, "Gotwo");
    let other = unsafe { libc::getppid() } as i64;

    assert!(
        app.request(Request::Give {
            pid: other,
            pane: Some("%9".into()),
            key: "another".into(),
        })
        .starts_with("error"),
        "only the note open here"
    );
    let reply = app.request(Request::Give {
        pid: other,
        pane: Some("%9".into()),
        key: key.clone(),
    });
    assert_eq!(reply, "ok");
    assert!(fs::read_to_string(&plan).unwrap().contains("two"), "saved");
    assert!(!lock.exists(), "let go");
    assert!(matches!(app.editor.mode, Mode::Normal));
    let held = app.held.as_ref().expect("read-only here");
    assert_eq!(held.holder.pid, other);
    assert_eq!(held.holder.pane.as_deref(), Some("%9"));

    // The other app takes it; this one doesn't take it back.
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.held.as_mut().unwrap().checked = Instant::now() - HELD_CHECK;
    app.tick();
    assert!(app.held.is_some());
}

#[test]
fn notes_another_app_has_open_keep_their_own_text_and_files() {
    let (_dir, store) = temp("held-others");
    let a = store.absolute("A.md");
    let b = store.absolute("B.md");
    write(&a, "# A\n\nthe first\n");
    write(&b, "# B\n\nthe second\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    app.open_note(&a, false, false);
    app.open_note(&b, false, false);
    let other = unsafe { libc::getppid() } as i64;
    for path in [&a, &b] {
        let key = id::in_text(&fs::read_to_string(path).unwrap()).unwrap();
        fs::write(
            app.store.root().join(store::LOCKS).join(key),
            format!("{other} %9"),
        )
        .unwrap();
    }
    app.open_note(&a, false, false);
    assert!(app.held.is_some());

    // Opening the other from the list shows its own text, not A's.
    app.reveal(&b);
    app.focus = Focus::List;
    keys(&mut app, "<CR>");
    assert_eq!(app.current_path(), Some(b.as_path()));
    assert_eq!(lines(&app), ["# B", "", "the second"]);

    // Deleting it from the list is refused while the other app has it.
    app.reveal(&a);
    app.focus = Focus::List;
    app.delete();
    assert!(a.is_file(), "left where it is");
    assert!(
        app.message
            .as_ref()
            .is_some_and(|(m, _)| m.contains("open in another Notes"))
    );
}

#[test]
fn a_squash_keeps_its_notes_locked_while_another_note_is_opened() {
    let (_dir, store) = temp("squash-locks");
    let a = store.absolute("A.md");
    let b = store.absolute("B.md");
    let c = store.absolute("C.md");
    for (path, title) in [(&a, "A"), (&b, "B"), (&c, "C")] {
        write(path, &format!("# {title}\n"));
    }
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    for path in [&c, &b, &a] {
        app.open_note(path, false, false);
    }
    let lock = |path: &Path| {
        let key = id::in_text(&fs::read_to_string(path).unwrap()).unwrap();
        app.store.root().join(store::LOCKS).join(key)
    };
    let (lock_a, lock_b) = (lock(&a), lock(&b));
    assert!(app.lock_for_squash(&[a.clone(), b.clone()]));
    app.open_note(&c, false, false);
    assert!(lock_a.exists(), "the open note's lock kept for the squash");
    assert!(lock_b.exists());
    app.unlock_squashed();
    assert!(!lock_a.exists() && !lock_b.exists(), "let go as it ends");
}

#[test]
fn a_squashed_note_never_opened_is_locked_by_the_id_opening_it_takes() {
    let (_dir, store) = temp("squash-new-id");
    let d = store.absolute("D.md");
    write(&d, "# D\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    assert!(app.lock_for_squash(std::slice::from_ref(&d)));
    let key = id::in_text(&fs::read_to_string(&d).unwrap()).expect("given its ID");
    assert!(app.store.root().join(store::LOCKS).join(key).exists());
    app.unlock_squashed();
}

#[test]
fn a_notes_lock_follows_its_id_through_an_outside_rewrite() {
    let (_dir, store) = temp("lock-follows-id");
    let a = store.absolute("A.md");
    write(&a, "# A\n\none\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    let key = id::in_text(&fs::read_to_string(&a).unwrap()).unwrap();
    let locks = app.store.root().join(store::LOCKS);

    // Rewritten without its ID: written back at once, its lock kept.
    write(&a, "# A\n\none\ntwo\n");
    app.check();
    app.tick();
    assert_eq!(
        id::in_text(&fs::read_to_string(&a).unwrap()).as_deref(),
        Some(key.as_str())
    );
    assert!(locks.join(&key).exists());

    // Given another outside: locked under that one, the old let go.
    let other = id::with("# A\n\none\n", &id::new());
    write(&a, &other);
    app.check();
    let new = id::in_text(&other).unwrap();
    assert!(locks.join(&new).exists());
    assert!(!locks.join(&key).exists());
}

#[test]
fn a_held_note_renamed_by_the_app_that_has_it_is_followed() {
    let (_dir, store) = temp("held-renamed");
    let a = store.absolute("A.md");
    write(&a, "# A\n\none\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    let text = fs::read_to_string(&a).unwrap();
    let key = id::in_text(&text).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    let other = unsafe { libc::getppid() } as i64;
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.open_note(&a, false, false);
    assert!(app.held.is_some());

    // The other app retitles it: saved whole into a new file, renamed.
    let renamed = app.store.absolute("B.md");
    write(&renamed, &text.replacen("# A", "# B", 1));
    fs::remove_file(&a).unwrap();
    app.check();
    assert_eq!(app.current_path(), Some(renamed.as_path()));

    fs::write(&lock, "999999999 %9").unwrap();
    app.held.as_mut().unwrap().checked = Instant::now() - HELD_CHECK;
    app.tick();
    assert!(app.held.is_none());
    keys(&mut app, "Gohello<Esc>");
    app.save();
    assert!(fs::read_to_string(&renamed).unwrap().contains("hello"));
}

#[test]
fn a_held_note_given_another_id_waits_on_that_ids_lock() {
    let (_dir, store) = temp("held-new-id");
    let a = store.absolute("A.md");
    write(&a, "# A\n\none\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    let key = id::in_text(&fs::read_to_string(&a).unwrap()).unwrap();
    let other = unsafe { libc::getppid() } as i64;
    let locks = app.store.root().join(store::LOCKS);
    fs::write(locks.join(&key), format!("{other} %9")).unwrap();
    app.open_note(&a, false, false);
    assert!(app.held.is_some());
    let new = id::new();
    write(&a, &id::with("# A\n\none\n", &new));
    app.check();
    assert_eq!(
        app.held.as_ref().map(|h| h.key.as_str()),
        Some(new.as_str())
    );
}

#[test]
fn undo_on_a_held_note_keeps_its_history_for_when_it_is_this_apps_again() {
    let (_dir, store) = temp("held-undo");
    let a = store.absolute("A.md");
    write(&a, "# A\n\none\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    keys(&mut app, "Gotwo<Esc>");
    app.save();
    let key = id::in_text(&fs::read_to_string(&a).unwrap()).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    let other = unsafe { libc::getppid() } as i64;
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.open_note(&a, false, false);
    assert!(app.held.is_some());
    keys(&mut app, "u");
    assert_eq!(lines(&app), ["# A", "", "one", "two"], "taken back");

    fs::write(&lock, "999999999 %9").unwrap();
    app.held.as_mut().unwrap().checked = Instant::now() - HELD_CHECK;
    app.tick();
    assert!(app.held.is_none());
    keys(&mut app, "u");
    assert_eq!(
        lines(&app),
        ["# A", "", "one"],
        "the edit still undone here"
    );
}

#[test]
fn a_held_note_renamed_as_its_app_lets_it_go_is_followed() {
    let (_dir, store) = temp("held-renamed-released");
    let a = store.absolute("A.md");
    write(&a, "# A\n\none\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    let text = fs::read_to_string(&a).unwrap();
    let key = id::in_text(&text).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    let other = unsafe { libc::getppid() } as i64;
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.open_note(&a, false, false);
    assert!(app.held.is_some());

    // Retitled and let go before this app looked again.
    let renamed = app.store.absolute("B.md");
    write(&renamed, &text.replacen("# A", "# B", 1));
    fs::remove_file(&a).unwrap();
    fs::remove_file(&lock).unwrap();
    app.held.as_mut().unwrap().checked = Instant::now() - HELD_CHECK;
    app.tick();
    assert!(app.held.is_none());
    assert_eq!(app.current_path(), Some(renamed.as_path()));
}

#[test]
fn add_an_item_is_refused_on_a_note_another_app_has_open() {
    let (_dir, store) = temp("held-add");
    let a = store.absolute("A.md");
    write(&a, "# A\n\n- [ ] one\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ux2: true,
            ..options()
        },
    );
    app.start();
    let key = id::in_text(&fs::read_to_string(&a).unwrap()).unwrap();
    let other = unsafe { libc::getppid() } as i64;
    fs::write(
        app.store.root().join(store::LOCKS).join(key),
        format!("{other} %9"),
    )
    .unwrap();
    app.open_note(&a, false, false);
    assert!(app.held.is_some());
    app.start_adding();
    assert!(app.adding.is_none(), "no input opened");
    app.adding = Some("two".into());
    app.add_item(true);
    assert!(app.planning.is_empty(), "nothing handed on");
}

#[test]
fn with_ux2_the_list_has_its_own_top_line_and_a_click_on_search_searches() {
    let (_dir, mut app) = started_ux2("top line");
    let shown = screen(&mut app);
    assert!(shown[0].starts_with(" Notes "), "{:?}", shown[0]);
    let list_w = app.geo.list.width as usize;
    let top: String = shown[0].chars().take(list_w).collect();
    assert!(top.trim_end().ends_with("/ search"), "{top:?}");
    assert_eq!(app.geo.list.y, 2, "the list under it and a blank row");
    let (x, y) = find(&shown, "/ search");
    app.focus = Focus::Note;
    click(&mut app, x, y);
    assert_eq!(app.focus, Focus::List, "the results take the keys");
    let mut buf = Buffer::empty(Rect::new(0, 0, app.cols, app.rows));
    app.draw(&mut buf);
    let all: String = buf.content.iter().map(|c| c.symbol()).collect();
    assert!(all.contains("Search notes"), "the search asked for");

    // Without UX 2.0, the list starts at the top.
    let (_dir, mut app) = started("no top line");
    let shown = screen(&mut app);
    assert!(!shown[0].contains("/ search"), "{:?}", shown[0]);
    assert_eq!(app.geo.list.y, 0);
}

#[test]
fn a_plan_line_too_long_for_its_chips_stays_and_a_sub_item_keeps_the_plan_chip() {
    let (_dir, mut app) = started_ux2("long choice");
    let path = app.store.create("", Some("Long"));
    let long = "keep the sidebar as it is or replace it ".repeat(4);
    write(
        &path,
        &format!(
            "# Long\n\n- [ ] A condensed sidebar\n  Plan: [[Plan - Condensed]] (needs a choice: {long})\n- [ ] Stable IDs\n  - [ ] a sub-item\n  Plan: [[Plan - IDs]]\n"
        ),
    );
    app.open_note(&path, false, false);
    app.editor.doc.row = 1;
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("Plan: Plan - Condensed")),
        "the line kept: {shown:#?}"
    );
    let at = shown
        .iter()
        .position(|l| l.contains("A condensed sidebar"))
        .unwrap();
    assert!(
        shown[at + 1].trim_end().ends_with("plan"),
        "the plan chip still drawn, past the choice too wide for the row: {shown:#?}"
    );
    assert!(
        !shown.iter().any(|l| l.contains("Plan: Plan - IDs")),
        "the sub-item's parent's plan line as chips: {shown:#?}"
    );
    assert!(shown.iter().any(|l| l.trim_end().ends_with("  plan")));
}

#[test]
fn typing_before_a_click_on_search_is_one_step_to_undo() {
    let (_dir, mut app) = started_ux2("search undo");
    let path = app.store.create("", Some("Typed"));
    write(&path, "# Typed\n\nfirst\n");
    app.open_note(&path, false, false);
    app.focus = Focus::Note;
    keys(&mut app, "3GA more");
    let shown = screen(&mut app);
    let (x, y) = find(&shown, "/ search");
    click(&mut app, x, y);
    keys(&mut app, "<Esc>");
    app.focus = Focus::Note;
    keys(&mut app, "u");
    assert_eq!(lines(&app)[2], "first", "what was typed undone");
}

#[test]
fn a_plan_line_in_a_visual_selection_shows_its_text() {
    let (_dir, mut app) = started_ux2("visual plan line");
    chips_note(&mut app);
    app.focus = Focus::Note;
    keys(&mut app, "3GVjj");
    assert_eq!(app.editor.doc.row, 4, "the cursor past the plan line");
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("Plan: Plan - Logo")),
        "{shown:#?}"
    );
}

#[test]
fn a_plan_line_linking_a_picture_keeps_its_text() {
    let (_dir, mut app) = started_ux2("picture plan line");
    let path = app.store.create("", Some("Pictured"));
    write(
        &path,
        "# Pictured\n\n- [ ] Draw it\n  Plan: [[Plan - Draw]] ![preview](preview.png)\n- [ ] Next\n",
    );
    app.open_note(&path, false, false);
    app.editor.doc.row = 4;
    let shown = screen(&mut app);
    assert!(
        shown.iter().any(|l| l.contains("Plan: Plan - Draw")),
        "{shown:#?}"
    );
}

#[test]
fn the_wheel_moving_the_cursor_onto_a_plan_line_shows_its_text() {
    let (_dir, mut app) = started_ux2("wheel plan line");
    let path = app.store.create("", Some("Many"));
    let items: String = (0..40)
        .map(|k| format!("- [ ] item {k}\n  Plan: [[P {k}]] (chose A)\n"))
        .collect();
    write(&path, &format!("# Many\n\n{items}"));
    app.open_note(&path, false, false);
    app.focus = Focus::Note;
    keys(&mut app, "gg");
    screen(&mut app);
    let mut onto_plan_lines = 0;
    for _ in 0..8 {
        let r = app.geo.note;
        app.mouse(Mouse {
            kind: MouseKind::Press,
            button: MouseButton::WheelDown,
            col: r.x + 2,
            row: r.y + 2,
            mods: Mods::NONE,
        });
        let shown = screen(&mut app);
        let line = lines(&app)[app.editor.doc.row].clone();
        if let Some(rest) = line.trim_start().strip_prefix("Plan: [[") {
            onto_plan_lines += 1;
            let name = rest.split("]]").next().unwrap();
            assert!(
                shown.iter().any(|l| l.contains(&format!("Plan: {name}"))),
                "the cursor's plan line shown: {shown:#?}"
            );
        }
    }
    assert!(
        onto_plan_lines > 0,
        "the wheel put the cursor on a plan line"
    );
}

#[test]
fn with_ux2_the_list_top_line_has_commands_which_a_click_opens_the_palette_with() {
    let (_dir, mut app) = started_ux2("commands");
    let shown = screen(&mut app);
    let top: String = shown[0].chars().take(app.geo.list.width as usize).collect();
    assert!(top.contains("⌘ Commands  / search"), "{top:?}");
    let (x, y) = find(&shown, "⌘ Commands");
    click(&mut app, x, y);
    assert_eq!(app.layers.len(), 1, "the palette open");

    // Off in Settings, it isn't drawn.
    let (_dir, mut app) = started_ux2("no commands");
    app.opts.commands = false;
    let shown = screen(&mut app);
    assert!(!shown[0].contains("Commands"), "{:?}", shown[0]);
}

#[test]
fn with_the_list_put_away_commands_sits_in_the_top_right_corner() {
    let (_dir, mut app) = started_ux2("corner");
    let path = app.store.create("", Some("Corner"));
    write(&path, "# Corner\n\nfirst\n");
    app.open_note(&path, false, false);
    app.list_shown = false;
    let shown = screen(&mut app);
    assert!(
        shown[0].trim_end().ends_with("⌘ Commands"),
        "{:?}",
        shown[0]
    );
    assert!(
        shown[0].contains('⋯'),
        "the toolbar beside it: {:?}",
        shown[0]
    );
    let (x, y) = find(&shown, "⌘ Commands");
    click(&mut app, x, y);
    assert_eq!(app.layers.len(), 1, "the palette open");
    app.layers.clear();

    // With no toolbar, the note starts a row down, under it.
    app.opts.toolbar = false;
    let shown = screen(&mut app);
    assert!(
        shown[0].trim_end().ends_with("⌘ Commands"),
        "{:?}",
        shown[0]
    );
    assert!(!shown[0].contains("Corner"), "{:?}", shown[0]);
    assert!(shown[1].contains("Corner"), "{:?}", shown[1]);
}

#[test]
fn with_no_note_open_and_the_list_put_away_commands_is_still_drawn() {
    let (_dir, mut app) = started_ux2("no note");
    app.open = None;
    app.list_shown = false;
    let shown = screen(&mut app);
    assert!(
        shown[0].trim_end().ends_with("⌘ Commands"),
        "{:?}",
        shown[0]
    );
}

#[test]
fn with_no_workspace_the_palette_leaves_out_what_only_it_does() {
    let names = |app: &App| -> Vec<String> {
        app.palette_commands(false)
            .into_iter()
            .map(|c| c.name)
            .collect()
    };
    // Alone, or outside a pane: its own commands, none of the workspace's.
    let (_dir, app) = started_ux2("palette-alone");
    let alone = names(&app);
    assert!(alone.iter().any(|n| n == "Find a note"));
    assert!(
        alone
            .iter()
            .any(|n| n == "Squash the marked notes into one")
    );
    for workspace in [
        "Open the note's tab",
        "Choose a tab's note",
        "Move Notes beside your work, or back to its own tab",
        "Search notes, Linear, GitHub and Slack",
        "Start work on this item in a new tab (wtn)",
    ] {
        assert!(!alone.iter().any(|n| n == workspace), "{workspace}");
    }
    // In a pane of the workspace, they are listed.
    let (_dir, store) = temp("palette-pane");
    let (mut app, _) = app_in(
        store,
        Options {
            server: true,
            ux2: true,
            ..options()
        },
    );
    app.start();
    let in_pane = names(&app);
    assert!(in_pane.iter().any(|n| n == "Open the note's tab"));
    assert!(
        in_pane
            .iter()
            .any(|n| n == "Start work on this item in a new tab (wtn)")
    );
}

#[test]
fn with_no_workspace_a_held_note_says_who_has_it_and_offers_nothing_of_a_tab_s() {
    let (_dir, store) = temp("held-alone");
    let plan = store.absolute("Plan.md");
    write(&plan, "# Plan\n\n- [ ] one\n");
    let (mut app, _) = app_in(
        store,
        Options {
            per_note: true,
            ..options()
        },
    );
    app.start();
    // The workspace's Notes holds it, in a pane this app can't reach.
    let key = id::in_text(&fs::read_to_string(&plan).unwrap()).unwrap();
    let lock = app.store.root().join(store::LOCKS).join(&key);
    let other = unsafe { libc::getppid() } as i64;
    fs::write(&lock, format!("{other} %9")).unwrap();
    app.open_note(&plan, false, false);

    keys(&mut app, "i");
    assert_eq!(app.top_kind(), None, "no box offering a tab or a takeover");
    assert!(matches!(app.editor.mode, Mode::Normal));
    assert_eq!(
        app.message.as_ref().map(|m| m.0.as_str()),
        Some("Open in another Notes, read-only here")
    );
    assert_eq!(lines(&app), ["# Plan", "", "- [ ] one"]);
}
