//! Vim's keys on a note: the shared editor (`switchboard_vim`, crates/vim),
//! which this app's editor was before it moved there so the code viewer and
//! the views read with the same Vim, with a note's own state beside its text
//! (`format::Note`) and its sections as the folds (`fold::Folds`). What a
//! note adds on top (Enter ticking a box, the Format menu, the leader keys,
//! tables, the plan) is the app's; this is the editing underneath, and its
//! tests are the notes app's own, run through the shared crate.

pub use switchboard_vim::editor::{Effect, History, Mode, Range, Register, recount};

/// The editor on a note.
pub type Editor = switchboard_vim::Editor<super::format::Note>;

/// A note's history and folds while another is open (`Editor::put_away`).
pub type Kept = switchboard_vim::editor::Kept<super::fold::Folds>;

#[cfg(test)]
use super::text::Text;
#[cfg(test)]
use switchboard_ui::input::{Key, KeyCode};

#[cfg(test)]
mod tests {
    use super::*;
    use switchboard_ui::input::Mods;

    fn editor(lines: &[&str]) -> Editor {
        Editor::new(Text::from_lines(lines))
    }

    /// Types keys as Vim's notation writes them: plain characters, and
    /// <Esc>, <CR>, <BS>, <C-r> and the like.
    fn press(e: &mut Editor, keys: &str) {
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
                    "BS" => Key::new(KeyCode::Backspace, Mods::NONE),
                    "Tab" => Key::new(KeyCode::Tab, Mods::NONE),
                    "S-Right" => Key::new(KeyCode::Right, Mods::SHIFT),
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
            e.begin();
            e.key(key);
            e.end();
        }
    }

    fn lines(e: &Editor) -> Vec<String> {
        e.doc.text.lines()
    }

    #[test]
    fn a_new_count_replaces_the_changes_count_whole() {
        let keys = |s: &str| -> Vec<Key> {
            s.chars()
                .map(|c| match c {
                    '⎋' => Key::new(KeyCode::Esc, Mods::NONE),
                    '→' => Key::new(KeyCode::Right, Mods::SHIFT),
                    '↵' => Key::new(KeyCode::Enter, Mods::NONE),
                    c => Key::new(KeyCode::Char(c), Mods::NONE),
                })
                .collect()
        };
        for (before, after) in [
            ("2dw", "4dw"),
            ("d3w", "4dw"),
            ("2d3w", "4dw"),
            ("x", "4x"),
            ("A!⎋", "4A!⎋"),
            ("g~2w", "4g~w"),
            ("d0", "4d0"),
        ] {
            assert_eq!(recount(&keys(before), 4), Some(keys(after)), "{before}");
        }
        let ctrl_a = [Key::new(KeyCode::Char('a'), Mods::CTRL)];
        let mut four_ctrl_a = keys("4");
        four_ctrl_a.extend(ctrl_a);
        assert_eq!(recount(&ctrl_a, 4), Some(four_ctrl_a));
        for kept in ["→x⎋", "↵", " b", "I!⎋", "a!⎋", "i!⎋", "\"+p", "vex", ""] {
            assert_eq!(recount(&keys(kept), 4), None, "{kept}");
        }
    }

    #[test]
    fn take_count_takes_only_a_count() {
        let mut e = editor(&["one"]);
        press(&mut e, "12");
        assert_eq!(e.take_count(), Some(12));
        assert!(!e.pending());
        press(&mut e, "2d");
        assert_eq!(e.take_count(), None);
        assert!(e.pending(), "the operator left waiting");
    }

    #[test]
    fn motions_move_as_vims_do() {
        let mut e = editor(&["alpha beta-gamma delta", "", "  second line here"]);
        press(&mut e, "w");
        assert_eq!(e.col(), 6);
        press(&mut e, "w");
        assert_eq!(e.col(), 10, "punctuation is a word of its own");
        press(&mut e, "W");
        assert_eq!(e.col(), 17);
        press(&mut e, "w");
        assert_eq!((e.row(), e.col()), (1, 0), "an empty line is a word");
        press(&mut e, "w");
        assert_eq!((e.row(), e.col()), (2, 2));
        press(&mut e, "b");
        assert_eq!((e.row(), e.col()), (1, 0));
        press(&mut e, "gg");
        press(&mut e, "e");
        assert_eq!(e.col(), 4);
        press(&mut e, "$");
        assert_eq!(e.col(), 21);
        press(&mut e, "0fd");
        assert_eq!(e.col(), 17);
        press(&mut e, "0;");
        assert_eq!(e.col(), 17, "; finds the same again");
        press(&mut e, "G");
        assert_eq!(
            (e.row(), e.col()),
            (2, 2),
            "G goes to the last line's first word"
        );
        press(&mut e, "2G");
        assert_eq!(e.row(), 1);
        press(&mut e, "kll");
        assert_eq!(e.col(), 2);
        press(&mut e, "j");
        assert_eq!((e.row(), e.col()), (1, 0));
        press(&mut e, "j");
        assert_eq!(
            e.col(),
            2,
            "j keeps the column it wants across a shorter line"
        );
    }

    #[test]
    fn ctrl_a_and_ctrl_x_change_the_number_at_or_after_the_cursor() {
        let mut e = editor(&["item 9 of 10", "no number", "-1 and 007", "on 2026-11-12"]);
        press(&mut e, "<C-a>");
        assert_eq!(
            e.doc.line(0),
            "item 10 of 10",
            "the first number after the cursor"
        );
        assert_eq!(e.col(), 6, "the cursor on its last digit");
        press(&mut e, "w");
        press(&mut e, "w");
        press(&mut e, "5<C-x>");
        assert_eq!(e.doc.line(0), "item 10 of 5", "a count subtracts that many");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "item 10 of 10", "one step to undo");
        press(&mut e, "j<C-a>");
        assert_eq!(
            e.doc.line(1),
            "no number",
            "a line with no number is left alone"
        );
        press(&mut e, "j0<C-a>");
        assert_eq!(
            e.doc.line(2),
            "0 and 007",
            "a sign at the line's start, and 0 has none"
        );
        press(&mut e, "<C-x>");
        assert_eq!(e.doc.line(2), "-1 and 007");
        press(&mut e, "w<C-a>");
        assert_eq!(e.doc.line(2), "-1 and 008", "leading zeros keep the width");
        press(&mut e, "j0f-<C-a>");
        assert_eq!(e.doc.line(3), "on 2026-12-12", "a date's dash is no sign");

        let mut e = editor(&["heading", "item 9", "abcdefgh"]);
        press(&mut e, "j<C-a>j");
        assert_eq!(
            (e.row(), e.col()),
            (2, 6),
            "the next line down keeps the number's column"
        );
    }

    #[test]
    fn operators_take_counts_motions_and_text_objects_and_undo_takes_them_back() {
        let mut e = editor(&["one two three four", "five", "six"]);
        press(&mut e, "dw");
        assert_eq!(e.doc.line(0), "two three four");
        press(&mut e, "2dw");
        assert_eq!(e.doc.line(0), "four");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "two three four");
        press(&mut e, "u");
        assert_eq!(
            e.doc.line(0),
            "one two three four",
            "each command undone on its own"
        );
        press(&mut e, "<C-r>");
        assert_eq!(e.doc.line(0), "two three four");
        press(&mut e, "wciwTHREE<Esc>");
        assert_eq!(e.doc.line(0), "two THREE four");
        press(&mut e, "u");
        assert_eq!(
            e.doc.line(0),
            "two three four",
            "a change and what was typed are one step"
        );
        press(&mut e, "jdd");
        assert_eq!(lines(&e), ["two three four", "six"]);
        press(&mut e, "p");
        assert_eq!(
            lines(&e),
            ["two three four", "six", "five"],
            "a deleted line put back under"
        );
        press(&mut e, "ggyyP");
        assert_eq!(lines(&e)[0..2], ["two three four", "two three four"]);
        press(&mut e, "3x");
        assert_eq!(e.doc.line(0), " three four");
        press(&mut e, "A!<Esc>");
        assert_eq!(e.doc.line(0), " three four!");
        press(&mut e, "0daw");
        assert_eq!(e.doc.line(0), " four!", "a word and the blank after it");
        press(&mut e, ">>");
        assert_eq!(e.doc.line(0), "   four!", "indented by shiftwidth");
        press(&mut e, "gUU");
        assert_eq!(e.doc.line(0), "   FOUR!");
    }

    #[test]
    fn insert_mode_types_joins_and_carries_the_indent() {
        let mut e = editor(&["  item"]);
        press(&mut e, "A one<CR>two<Esc>");
        assert_eq!(
            lines(&e),
            ["  item one", "  two"],
            "Enter keeps the line's indent"
        );
        press(&mut e, "I<BS><BS><BS><Esc>");
        assert_eq!(
            lines(&e),
            ["  item onetwo"],
            "Backspace at a line's start joins it to the one above"
        );
        press(&mut e, "otext (inside) end<Esc>F(di(");
        assert_eq!(e.doc.line(1), "  text () end");
        press(&mut e, "ccnew<Esc>");
        assert_eq!(e.doc.line(1), "  new", "cc keeps the indent");
        press(&mut e, "kJ");
        assert_eq!(lines(&e), ["  item onetwo new"]);
    }

    #[test]
    fn replace_mode_types_over_and_backspace_puts_back() {
        let mut e = editor(&["hello"]);
        press(&mut e, "Rab");
        assert_eq!(e.mode, Mode::Replace);
        press(&mut e, "<Esc>");
        assert_eq!(e.doc.line(0), "abllo");
        assert_eq!((e.mode, e.col()), (Mode::Normal, 1), "the cursor on b");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "hello", "the whole of it one step to undo");
        press(&mut e, "0Rab<BS><BS><Esc>");
        assert_eq!(e.doc.line(0), "hello", "Backspace puts back what was there");
        press(&mut e, "0l<BS>Rx<BS><BS>y<Esc>");
        assert_eq!(
            e.doc.line(0),
            "yello",
            "Backspace past where R started only moves left"
        );
        press(&mut e, "u$Rxyz<Esc>");
        assert_eq!(e.doc.line(0), "hellxyz", "past the end it adds");
        press(&mut e, "u0Rab<CR>cd<Esc>");
        assert_eq!(
            lines(&e),
            ["ab", "cdo"],
            "Enter breaks the line, taking nothing"
        );
        press(&mut e, "u");

        let mut e = editor(&["abcdef"]);
        press(&mut e, "RX");
        e.doc.col = 4;
        press(&mut e, "<BS><Esc>");
        assert_eq!(
            e.doc.line(0),
            "Xbcdef",
            "moved by a click, Backspace puts back nothing it didn't type over"
        );

        let mut clicked = editor(&["one", "two"]);
        press(&mut clicked, "3RX");
        clicked.doc.row = 1;
        clicked.doc.col = 0;
        clicked.settle_replace_count();
        assert_eq!(
            lines(&clicked),
            ["Xne", "two"],
            "nothing typed again after a click"
        );

        let mut down = editor(&["abcdef", "abcdef", "abcdef"]);
        press(&mut down, "R");
        for code in [KeyCode::Down, KeyCode::Right, KeyCode::Right, KeyCode::Down] {
            down.key(Key::new(code, Mods::NONE));
        }
        press(&mut down, "X<Esc>");
        assert_eq!(
            down.doc.line(2),
            "abXdef",
            "the arrows' column, not one kept from before"
        );

        let mut e = editor(&["0123456789"]);
        press(&mut e, "3Rab<Esc>");
        assert_eq!(e.doc.line(0), "ababab6789", "a count types it again");
        assert_eq!(e.col(), 5);
    }

    #[test]
    fn insert_modes_ctrl_keys_put_a_register_run_a_command_and_indent() {
        let mut e = editor(&["alpha beta"]);
        press(&mut e, "yiwA <C-r>\"<Esc>");
        assert_eq!(e.doc.line(0), "alpha beta alpha", "the register typed in");
        e.search_for("beta", true);
        press(&mut e, "0i<C-r>/<Esc>");
        assert_eq!(e.doc.line(0), "betaalpha beta alpha", "the last search");
        press(&mut e, "u");
        press(&mut e, "i<C-r>z<Esc>");
        assert_eq!(
            e.doc.line(0),
            "alpha beta alpha",
            "an unknown register types nothing"
        );
        assert!(
            e.effects()
                .iter()
                .any(|m| matches!(m, Effect::Message(t) if t.starts_with("E353"))),
            "and says so"
        );

        let mut e = editor(&["one", "two"]);
        press(&mut e, "A<C-o>");
        assert_eq!(e.mode, Mode::Normal);
        assert!(e.one_command());
        press(&mut e, "0");
        assert_eq!((e.mode, e.col()), (Mode::Insert, 0), "back to typing");
        press(&mut e, "X<C-o><Esc>Y<Esc>");
        assert_eq!(e.doc.line(0), "XYone");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "Xone", "Ctrl-O breaks the step to undo");
        press(&mut e, "u");
        assert_eq!(e.doc.line(0), "one");
        let mut gone = editor(&["abc", "x"]);
        press(&mut gone, "A<C-o>ddX<Esc>");
        assert_eq!(
            lines(&gone),
            ["Xx"],
            "typing goes on where the command left it"
        );
        let mut ex = editor(&["a"]);
        press(&mut ex, "A<C-o>:");
        assert!(ex.one_command(), "waiting for the prompt");
        ex.ex("s/a//");
        ex.prompt_done();
        press(&mut ex, "<Esc>");
        assert_eq!((lines(&ex), ex.mode), (vec![String::new()], Mode::Normal));
        let mut dd = editor(&["one", "two"]);
        press(&mut dd, "i<C-o>dd<Esc>u");
        assert_eq!(lines(&dd), ["one", "two"], "a command under Ctrl-O undoes");
        press(&mut e, "AZ<C-o>u");
        assert_eq!(
            (e.doc.line(0).as_str(), e.mode),
            ("one", Mode::Insert),
            "u under Ctrl-O undoes what was just typed"
        );
        press(&mut e, "<Esc>");
        let mut named = editor(&["word"]);
        press(&mut named, "yiwA <C-o>\"+p");
        assert_eq!(
            (named.doc.line(0).as_str(), named.mode),
            ("word word", Mode::Insert),
            "a register named is part of the one command"
        );
        press(&mut named, "<Esc>");
        press(&mut e, "A<C-o>l!<Esc>");
        assert_eq!(
            e.doc.line(0),
            "one!",
            "a command that doesn't move keeps the cursor past the end"
        );
        press(&mut e, "i<C-o>dd-<Esc>");
        assert_eq!(lines(&e), ["-two"], "the line deleted and typing kept on");

        let mut e = editor(&["- item", "", "yy"]);
        press(&mut e, "llli<C-t>");
        assert_eq!(
            (e.doc.line(0).as_str(), e.col()),
            ("  - item", 5),
            "on the same character"
        );
        press(&mut e, "<C-d><C-d><Esc>");
        assert_eq!(
            e.doc.line(0),
            "- item",
            "and back, nothing past the line's start"
        );
        press(&mut e, "jji<C-t><Esc>");
        assert_eq!(e.doc.line(2), "  yy");
        press(&mut e, "ggyyGo<C-r>\"<Esc>");
        assert_eq!(
            lines(&e)[3..],
            ["  - item", ""],
            "a line put with its newline"
        );
    }

    #[test]
    fn block_visual_mode_acts_on_a_rectangle() {
        let text = ["abcdef", "a", "abcdefgh", "abcd"];
        let mut e = editor(&text);
        press(&mut e, "l<C-v>jjl");
        assert!(matches!(e.mode, Mode::Visual { block: true, .. }));
        press(&mut e, "d");
        assert_eq!(
            lines(&e),
            ["adef", "a", "adefgh", "abcd"],
            "two columns cut, a row short of them left alone"
        );
        assert_eq!((e.row(), e.col()), (0, 1), "the cursor on the top-left");
        assert_eq!(e.register.text, "bc\n\nbc");
        press(&mut e, "u");
        assert_eq!(lines(&e), text);

        press(&mut e, "gg0l<C-v>jjly$p");
        assert_eq!(
            lines(&e)[..3],
            ["abcdefbc", "a     ", "abcdefbcgh"],
            "put as a column, a short row padded to reach it"
        );
        press(&mut e, "u");
        press(&mut e, "gg0<C-v>jrx");
        assert_eq!(lines(&e)[..2], ["xbcdef", "x"]);
        press(&mut e, "u");
        press(&mut e, "gg0l<C-v>jj>");
        assert_eq!(
            lines(&e)[..3],
            ["a  bcdef", "a", "a  bcdefgh"],
            "shifted at the block's column"
        );
        press(&mut e, "u");
        press(&mut e, "gg0l<C-v>jjI- <Esc>");
        assert_eq!(
            lines(&e)[..3],
            ["a- bcdef", "a", "a- bcdefgh"],
            "typed into every line"
        );
        press(&mut e, "u");
        assert_eq!(lines(&e), text, "one step to undo");
        press(&mut e, "gg0<C-v>jjj$A;<Esc>");
        assert_eq!(
            lines(&e),
            ["abcdef;", "a;", "abcdefgh;", "abcd;"],
            "each line's end"
        );
        press(&mut e, "u");
        press(&mut e, "gg0l<C-v>jjlc_<Esc>");
        assert_eq!(
            lines(&e)[..3],
            ["a_def", "a", "a_defgh"],
            "changed on each row"
        );
        press(&mut e, "u");

        let mut short = editor(&["abcdef", "xy"]);
        press(&mut short, "3l<C-v>j$d");
        assert_eq!(
            lines(&short),
            ["abc", "xy"],
            "$ keeps the block's left edge"
        );
        let mut short = editor(&["a", "abcdef"]);
        press(&mut short, "j2l<C-v>kIX<Esc>");
        assert_eq!(
            lines(&short),
            ["a", "abXcdef"],
            "typing starts on a row that reaches the block"
        );
        let mut moved = editor(&["abc", "abc"]);
        press(&mut moved, "<C-v>jIX");
        moved.key(Key::new(KeyCode::Right, Mods::NONE));
        press(&mut moved, "<Esc>");
        assert_eq!(
            lines(&moved),
            ["Xabc", "abc"],
            "moving ends typing into every row"
        );
        let mut switched = editor(&["abcd"]);
        press(&mut switched, "$v<C-v>d");
        assert_eq!(
            lines(&switched),
            ["abc"],
            "v then Ctrl-V keeps the block's column"
        );
        let mut clicked = editor(&["abc", "abc"]);
        press(&mut clicked, "<C-v>jIX");
        clicked.doc.col = 3;
        press(&mut clicked, "<Esc>");
        assert_eq!(
            lines(&clicked),
            ["Xabc", "abc"],
            "a click ends typing into every row"
        );
        let mut indented = editor(&["abcd", "abcd"]);
        press(&mut indented, "ll<C-v>jI<C-t>X<Esc>");
        assert_eq!(lines(&indented), ["  abXcd", "abcd"]);
        let mut ragged = editor(&["abc", "d", "xy", "xy"]);
        press(&mut ragged, "0<C-v>lljy");
        press(&mut ragged, "jj0l2P");
        assert_eq!(
            lines(&ragged)[2..],
            ["xabcabcy", "xd  d  y"],
            "each copy of a count padded to the block's width"
        );
        let mut pasted = editor(&["abc", "abc"]);
        press(&mut pasted, "<C-v>jI");
        pasted.paste_text("X");
        press(&mut pasted, "<Esc>");
        assert_eq!(
            lines(&pasted),
            ["Xabc", "Xabc"],
            "a paste goes into every row"
        );
        let mut back = editor(&["abcd", "abcd"]);
        press(&mut back, "ll<C-v>jI<BS>XY<Esc>");
        assert_eq!(
            lines(&back),
            ["aXYcd", "abcd"],
            "deleting back past the block's column keeps it to the row"
        );
        let mut corners = editor(&["abcdef", "a"]);
        press(&mut corners, "l<C-v>lljOd");
        assert_eq!(
            lines(&corners),
            ["aef", "a"],
            "O on a short row keeps the rectangle"
        );
        let mut corners = editor(&["abcdef", "a"]);
        press(&mut corners, "l<C-v>lljoOd");
        assert_eq!(lines(&corners), ["aef", "a"], "and so does o");
        let mut swap = editor(&["ab12", "cd34"]);
        press(&mut swap, "<C-v>jlyll<C-v>jlp");
        assert_eq!(
            lines(&swap),
            ["abab", "cdcd"],
            "p over a block at the lines' end"
        );
        let mut charwise = editor(&["a", "abcdef"]);
        charwise.register = Register {
            text: "X".into(),
            linewise: false,
            block: false,
        };
        press(&mut charwise, "j3l<C-v>kp");
        assert_eq!(
            lines(&charwise),
            ["a  X", "abcef"],
            "text put at the block's column"
        );
        let mut pasted = editor(&["abcd", "abcd"]);
        press(&mut pasted, "$<C-v>j");
        pasted.cut_to_type();
        pasted.type_text("X");
        assert_eq!(
            lines(&pasted),
            ["abcX", "abc"],
            "a paste over a block at its column"
        );
        let mut none = editor(&["abcdef", "a", "b"]);
        press(&mut none, "4l<C-v>jojjIX<Esc>");
        assert_eq!(
            lines(&none),
            ["abcdef", "a", "b"],
            "no row reaches the block"
        );
        let mut easy = editor(&["abc", "abc"]);
        easy.easy = true;
        press(&mut easy, "<C-v>jIX<S-Right><Esc>i<Esc>");
        assert_eq!(
            lines(&easy),
            ["Xabc", "abc"],
            "a selection ends typing into a block"
        );
        let mut shifted = editor(&["abc", "abc"]);
        press(&mut shifted, "<C-v>j<S-Right>d");
        assert_eq!(
            lines(&shifted),
            ["c", "c"],
            "Shift with an arrow widens a block"
        );
        let mut joined = editor(&["abc", "def", "ghi"]);
        press(&mut joined, "<C-v>jj$A");
        joined.key(Key::new(KeyCode::Delete, Mods::NONE));
        press(&mut joined, "X<Esc>");
        assert_eq!(
            lines(&joined),
            ["abcXdef", "ghi"],
            "a join ends typing into every row"
        );
        let mut cmd = editor(&["abc", "def"]);
        press(&mut cmd, "<C-v>jI<C-o>ddX<Esc>");
        assert_eq!(lines(&cmd), ["Xdef"], "Ctrl-O ends typing into every row");
        let mut two = editor(&["abc", "abc"]);
        press(&mut two, "<C-v>j$cX<Esc>");
        assert_eq!(
            lines(&two),
            ["X", "X"],
            "a row cut to nothing still takes it"
        );

        press(&mut e, "gg0l<C-v>jjlO");
        assert_eq!(
            (e.row(), e.col(), e.anchor),
            (2, 1, (0, 2)),
            "the other corner"
        );
        press(&mut e, "v");
        assert!(matches!(
            e.mode,
            Mode::Visual {
                block: false,
                line: false,
                ..
            }
        ));
        press(&mut e, "<Esc><C-v><C-v>");
        assert_eq!(e.mode, Mode::Normal, "Ctrl-V again leaves it");
    }

    #[test]
    fn linewise_visual_j_and_k_move_the_selected_lines() {
        let items = ["# List", "- [ ] one", "- [x] two", "  - [ ] three"];
        let mut e = editor(&items);
        press(&mut e, "jjVJ");
        assert_eq!(
            lines(&e),
            ["# List", "- [ ] one", "  - [ ] three", "- [x] two"]
        );
        assert_eq!(e.selected(), Some(Range::Lines(3, 3)), "still selected");
        press(&mut e, "K");
        assert_eq!(lines(&e), items);
        assert_eq!(e.selected(), Some(Range::Lines(2, 2)));
        press(&mut e, "<Esc>kVjK");
        assert_eq!(lines(&e), items, "nothing moves above the title");
        press(&mut e, "<Esc>VjK");
        assert_eq!(
            lines(&e),
            ["# List", "- [x] two", "  - [ ] three", "- [ ] one"]
        );
        assert_eq!(e.selected(), Some(Range::Lines(1, 2)), "both moved up");
        press(&mut e, "<Esc>GVJ");
        assert_eq!(lines(&e)[3], "- [ ] one", "the last line stays");
        press(&mut e, "<Esc>ggVJ");
        assert_eq!(lines(&e)[0], "# List", "the title stays");
    }

    #[test]
    fn j_and_k_pass_and_carry_a_closed_section_whole_and_hide_no_line_shown() {
        let mut e = editor(&["# T", "a", "## S", "x", "y", "## U", "z"]);
        press(&mut e, "jjzcggjVJ");
        assert_eq!(lines(&e), ["# T", "## S", "x", "y", "a", "## U", "z"]);
        assert_eq!(e.selected(), Some(Range::Lines(4, 4)));
        assert!(!e.folds.is_closed(1), "the section it fell in opened");
        let mut e = editor(&["# T", "## S", "## U", "z"]);
        press(&mut e, "jjzcVK");
        assert_eq!(lines(&e), ["# T", "## U", "z", "## S"]);
        assert!(e.folds.is_closed(1), "the section moved closed");
        assert_eq!(e.selected(), Some(Range::Lines(1, 1)), "its heading chosen");
    }

    #[test]
    fn a_count_moves_the_lines_that_far_and_undo_takes_one_move_back() {
        let mut e = editor(&["# T", "a", "b", "c", "d", "e"]);
        press(&mut e, "jV3J");
        assert_eq!(lines(&e), ["# T", "b", "c", "d", "a", "e"]);
        press(&mut e, "J");
        assert_eq!(lines(&e), ["# T", "b", "c", "d", "e", "a"]);
        press(&mut e, "9J");
        assert_eq!(lines(&e), ["# T", "b", "c", "d", "e", "a"], "at the end");
        press(&mut e, "<Esc>u");
        assert_eq!(
            lines(&e),
            ["# T", "b", "c", "d", "a", "e"],
            "one move undone"
        );
        press(&mut e, "Gvk$J");
        assert_eq!(e.mode, Mode::Normal);
        assert_eq!(
            lines(&e),
            ["# T", "b", "c", "d", "a e"],
            "v's J still joins"
        );
    }

    #[test]
    fn visual_mode_selects_and_acts_and_search_finds_with_smartcase() {
        let mut e = editor(&["alpha beta", "gamma Delta", "epsilon"]);
        press(&mut e, "wvey");
        assert_eq!(e.register.text, "beta");
        assert_eq!(e.mode, Mode::Normal);
        press(&mut e, "Vjd");
        assert_eq!(lines(&e), ["epsilon"]);
        press(&mut e, "u");
        assert_eq!(lines(&e).len(), 3);
        e.search_for("delta", true);
        assert_eq!((e.row(), e.col()), (1, 6), "lowercase matches either case");
        e.search_for("Delta", true);
        assert_eq!((e.row(), e.col()), (1, 6));
        e.search_for("eta", true);
        assert_eq!((e.row(), e.col()), (0, 7), "wrapping round to the top");
        press(&mut e, "n");
        assert_eq!((e.row(), e.col()), (0, 7));
        e.ex("%s/a/A/g");
        assert_eq!(e.doc.line(0), "AlphA betA");
        e.ex("3");
        assert_eq!(e.row(), 2);
    }

    #[test]
    fn easy_modes_shift_arrows_select_and_typing_replaces() {
        let mut e = editor(&["hello world"]);
        e.easy = true;
        press(&mut e, "<S-Right><S-Right><S-Right><S-Right>");
        assert!(matches!(e.mode, Mode::Visual { select: true, .. }));
        press(&mut e, "J");
        assert_eq!(
            e.doc.line(0),
            "J world",
            "a key typed over a selection replaces it"
        );
        assert_eq!(e.mode, Mode::Insert);
    }

    fn messages(e: &mut Editor) -> Vec<String> {
        e.effects()
            .into_iter()
            .filter_map(|x| match x {
                Effect::Message(m) => Some(m),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_mark_takes_the_cursor_back_to_its_place_or_its_line() {
        let mut e = editor(&["one", "  two words", "three", "four"]);
        press(&mut e, "jwlma");
        assert_eq!((e.row(), e.col()), (1, 3));
        press(&mut e, "G`a");
        assert_eq!((e.row(), e.col()), (1, 3), "back to the exact place");
        press(&mut e, "G'a");
        assert_eq!((e.row(), e.col()), (1, 2), "to the line's first non-blank");
        press(&mut e, "`b");
        assert_eq!(messages(&mut e), vec!["E20: Mark not set".to_string()]);
    }

    #[test]
    fn a_jump_into_a_closed_section_opens_it() {
        let mut e = editor(&["# T", "## A", "a1", "a2", "## B", "b1"]);
        press(&mut e, "3Gmakzc`a");
        assert_eq!(e.row(), 2);
        assert!(!e.folds.is_closed(1), "the section opened");
    }

    #[test]
    fn backticks_twice_go_back_where_the_cursor_jumped_from_and_forward_again() {
        let mut e = editor(&["one", "two", "three", "four"]);
        press(&mut e, "jlG");
        assert_eq!(e.row(), 3);
        press(&mut e, "``");
        assert_eq!((e.row(), e.col()), (1, 1));
        press(&mut e, "``");
        assert_eq!(e.row(), 3, "pressed again it comes back");
        press(&mut e, "''");
        assert_eq!((e.row(), e.col()), (1, 0), "'' to the line");
    }

    #[test]
    fn an_operator_reaches_a_mark() {
        let mut e = editor(&["one", "two", "three", "four"]);
        press(&mut e, "jmajjd'a");
        assert_eq!(lines(&e), vec!["one"]);
        let mut e = editor(&["one two three"]);
        press(&mut e, "wmawd`a");
        assert_eq!(lines(&e), vec!["one three"]);
    }

    #[test]
    fn undoing_a_deletion_brings_back_its_marks() {
        let mut e = editor(&["one", "two", "three"]);
        press(&mut e, "jmaddu'a");
        assert!(messages(&mut e).is_empty());
        assert_eq!(e.row(), 1);
        press(&mut e, "kJu'a");
        assert_eq!(e.row(), 1, "back on its own line once a join is undone");
        press(&mut e, "ggmaxjmau'a");
        assert_eq!(e.row(), 1, "a mark set again since stays where it was set");
        press(&mut e, "Gkddu``");
        assert!(messages(&mut e).is_empty(), "the context mark back too");
        assert_eq!(e.row(), 1);
    }

    #[test]
    fn a_mark_moves_with_its_line_and_goes_when_the_line_does() {
        let mut e = editor(&["one", "two", "three"]);
        press(&mut e, "jmaggOnew<Esc>G'a");
        assert_eq!(e.row(), 2, "a line put above moved it down");
        press(&mut e, "dd'a");
        assert_eq!(messages(&mut e), vec!["E20: Mark not set".to_string()]);
    }

    #[test]
    fn a_jump_says_where_it_left_and_a_step_does_not() {
        let mut e = editor(&["one", "two", "three"]);
        press(&mut e, "jl");
        assert!(e.effects().is_empty());
        press(&mut e, "G");
        assert_eq!(e.effects(), vec![Effect::Jump { from: (1, 1) }]);
        e.search_for("one", true);
        assert_eq!(e.effects(), vec![Effect::Jump { from: (2, 0) }]);
        press(&mut e, "dG");
        assert!(
            !e.effects().iter().any(|x| matches!(x, Effect::Jump { .. })),
            "an operator over a jump moves nothing"
        );
    }

    #[test]
    fn an_outside_change_while_nothing_is_typed_is_no_change_to_visit() {
        let mut e = editor(&["one", "two"]);
        press(&mut e, "i");
        e.change_outside(|doc| doc.text = Text::from_lines(&["one", "two", "three"]));
        press(&mut e, "<Esc>g;");
        assert_eq!(
            messages(&mut e),
            vec!["E664: changelist is empty".to_string()]
        );
        press(&mut e, "ix");
        e.change_outside(|doc| doc.text = Text::from_lines(&["xone", "two"]));
        press(&mut e, "<Esc>g;");
        assert!(messages(&mut e).is_empty(), "typing before it still counts");
    }

    #[test]
    fn a_change_made_during_a_key_is_one_to_visit() {
        let mut e = editor(&["one", "two"]);
        e.begin();
        e.change(|doc| doc.text = Text::from_lines(&["one", "TWO"]));
        e.end();
        press(&mut e, "g;");
        assert!(messages(&mut e).is_empty());
    }

    #[test]
    fn typing_before_insert_modes_ctrl_o_is_a_change_to_visit() {
        let mut e = editor(&["one", "two"]);
        press(&mut e, "iX<C-o>j<Esc>g;");
        assert!(messages(&mut e).is_empty());
        assert_eq!(e.row(), 0);
    }

    #[test]
    fn typing_under_way_as_the_note_is_put_away_is_a_change_to_visit() {
        let mut e = editor(&["one", "two", "three"]);
        press(&mut e, "jAx");
        let kept = e.put_away();
        e.take_back(Text::from_lines(&["one", "twox", "three"]), kept);
        press(&mut e, "ggg;");
        assert_eq!(e.row(), 1);
    }

    #[test]
    fn the_change_visited_stays_the_one_visited_when_earlier_ones_go() {
        let mut e = editor(&["aa", "bb", "cc", "dd", "ee"]);
        press(&mut e, "xjxjxjxjx2g;");
        assert_eq!(e.row(), 3);
        e.change_outside(|doc| doc.text = Text::from_lines(&["d", "e"]));
        press(&mut e, "g,");
        assert_eq!(e.row(), 1, "on to the newest, the one after it");
        let mut e = editor(&["aa", "bb", "cc"]);
        press(&mut e, "xjxjx3g;");
        assert_eq!(e.row(), 0);
        e.change_outside(|doc| doc.text = Text::from_lines(&["b", "c"]));
        press(&mut e, "g,");
        assert_eq!(e.row(), 0, "the oldest gone, on to the one after it");
        press(&mut e, "g,");
        assert_eq!(e.row(), 1);
        let mut e = editor(&["aa", "bb", "cc"]);
        press(&mut e, "xjxjx3g;");
        e.change_outside(|doc| doc.text = Text::from_lines(&["b", "c"]));
        press(&mut e, "ma");
        e.change_outside(|doc| doc.text = Text::from_lines(&["new", "b", "c"]));
        press(&mut e, "g,");
        assert_eq!(e.row(), 1, "still before the one after it, which moved");
    }

    #[test]
    fn g_semicolon_and_g_comma_visit_the_changes() {
        let rows: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let mut e = editor(&rows);
        press(&mut e, "g;");
        assert_eq!(
            messages(&mut e),
            vec!["E664: changelist is empty".to_string()]
        );
        press(&mut e, "2Gx3jx3jA!<Esc>gg");
        press(&mut e, "g;");
        assert_eq!(e.row(), 7);
        press(&mut e, "g;");
        assert_eq!(e.row(), 4);
        messages(&mut e);
        press(&mut e, "2g;");
        assert_eq!(e.row(), 1);
        assert_eq!(
            messages(&mut e),
            vec!["E662: At start of changelist".to_string()]
        );
        press(&mut e, "g,");
        assert_eq!(e.row(), 4);
        press(&mut e, "2g;99g,");
        assert_eq!(e.row(), 7, "a count past the newest goes to it");
        press(&mut e, "g;");
        messages(&mut e);
        press(&mut e, "g,g,");
        assert_eq!(e.row(), 7);
        assert_eq!(
            messages(&mut e),
            vec!["E663: At end of changelist".to_string()]
        );
        press(&mut e, "x");
        assert_eq!(
            e.changes.len(),
            3,
            "a change on the newest's line replaces it"
        );
        press(&mut e, "ggdd");
        assert_eq!(
            e.changes.iter().map(|c| c.0).collect::<Vec<_>>(),
            vec![0, 3, 6, 0],
            "every entry moved up with its line"
        );
    }
}
