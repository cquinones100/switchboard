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
        "# Plan\n\n- [ ] one (ABC-1)\n- [ ] two, typed\n- [ ] three (ABC-2)\n"
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
    let bin = dir.path.join(".bin");
    fs::create_dir_all(&bin).unwrap();
    let claude = bin.join("claude");
    fs::write(
        &claude,
        "#!/bin/sh\ncat >/dev/null\nsleep 1\nprintf '# Both\\n'\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    }
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
