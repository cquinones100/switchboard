// Tests for notes/ (the notes app) and bin/notes. Run with: node --test test/*.test.js
//
// Nothing here touches your real notes or Neovim config. Each test gets a
// notes directory of its own, HOME is a temporary directory, and Neovim runs
// headless with no config at all (-u NONE), the notes plugin added to its
// runtimepath. A test's Lua runs in that Neovim and prints its findings as
// JSON, which the test then checks.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawn } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const plugin = path.join(__dirname, "../notes");
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "notes-test-")));
const home = path.join(tmp, "home");
fs.mkdirSync(home);
after(() => fs.rmSync(tmp, { recursive: true, force: true }));
// Renaming or moving a note reaches the tmux server it runs in, to carry
// tabs' links along: never yours. A test that wants it starts one of its own.
delete process.env.TMUX;

let dirs = 0;
// Runs `code` in a headless Neovim with the plugin, in a fresh notes
// directory (or `dir`), and returns what it printed as JSON.
const lua = (code, dir = fs.mkdtempSync(path.join(tmp, `notes-${dirs++}-`))) => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${plugin}`, "--cmd", "runtime plugin/notes.lua", "-l", "/dev/stdin"], {
    input: code,
    encoding: "utf8",
    env: { ...process.env, HOME: home, NOTES_DIR: dir, XDG_CONFIG_HOME: path.join(home, ".config"), XDG_DATA_HOME: path.join(home, ".local/share"), XDG_STATE_HOME: path.join(home, ".local/state") },
  });
  return { result: JSON.parse(out.trim().split("\n").at(-1)), dir };
};

test("a note's title is its first line, its file is named for it, and renamed when it changes", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local a = s.create("", "Grocery list")
    local b = s.create("", "Grocery list")
    local blank = s.create("", nil)
    local f = io.open(a, "w"); f:write("# Weekend plans\\n\\nhike\\n"); f:close()
    local renamed = s.rename_to_title(a)
    local again = s.rename_to_title(renamed)
    io.write(vim.json.encode({
      a = s.relative(a), b = s.relative(b), blank = s.relative(blank), renamed = s.relative(renamed), again = s.relative(again),
      blank_title = s.title_of(io.open(blank):read("*a")),
      plain = s.title_of("\\n\\nJust words\\nmore"),
      preview = s.preview_of("# Title\\n\\n- [ ] eggs\\n**milk** and ==bread==\\n"),
    }))
  `);
  assert.equal(result.a, "Grocery list.md");
  assert.equal(result.b, "Grocery list 2.md", "a second note with that title doesn't overwrite the first");
  assert.equal(result.blank, "New Note.md");
  assert.equal(result.blank_title, "New Note", "an empty title reads as New Note");
  assert.equal(result.renamed, "Weekend plans.md");
  assert.equal(result.again, "Weekend plans.md", "a name that fits is left alone");
  assert.equal(result.plain, "Just words", "a first line without # is the title too");
  assert.equal(result.preview, "eggs milk and bread");
  assert.ok(fs.existsSync(path.join(dir, "Weekend plans.md")));
  assert.ok(!fs.existsSync(path.join(dir, "Grocery list.md")));
});

test("notes list pinned first, then newest; folders nest; Recently Deleted keeps a note until it's deleted there", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local old = s.create("", "Old")
    local new = s.create("Work/Projects", "Launch")
    local pinned = s.create("Work", "Pinned one")
    vim.uv.fs_utime(old, os.time() - 500, os.time() - 500)
    vim.uv.fs_utime(pinned, os.time() - 900, os.time() - 900)
    s.set_pinned(s.relative(pinned), true)
    local order = vim.tbl_map(function(n) return n.title end, s.notes())
    local work = vim.tbl_map(function(n) return n.title end, s.notes("Work"))
    local folders = vim.tbl_map(function(f) return { f.rel, f.depth, f.count } end, s.folders())
    -- Moved, a pinned note stays pinned.
    local moved = s.move(pinned, "")
    local still = s.is_pinned(s.relative(moved))
    local trashed = s.delete(new)
    local listed = #s.notes()
    local in_trash = vim.tbl_map(function(n) return n.rel end, s.notes(s.TRASH))
    local restored = s.restore(trashed)
    local gone = s.delete(s.delete(old))
    io.write(vim.json.encode({ order = order, work = work, folders = folders, still = still, trashed = s.relative(trashed), listed = listed, in_trash = in_trash, restored = s.relative(restored), gone = gone == nil, left = #s.notes(s.TRASH) }))
  `);
  assert.deepEqual(result.order, ["Pinned one", "Launch", "Old"]);
  assert.deepEqual(result.work, ["Pinned one", "Launch"], "a folder shows the notes in the folders under it too");
  assert.deepEqual(result.folders, [["Work", 0, 1], ["Work/Projects", 1, 1]]);
  assert.equal(result.still, true);
  assert.equal(result.trashed, ".trash/Work/Projects/Launch.md", "it remembers the folder it came from");
  assert.equal(result.listed, 2, "and is out of every other list");
  assert.deepEqual(result.in_trash, [".trash/Work/Projects/Launch.md"]);
  assert.equal(result.restored, "Work/Projects/Launch.md");
  assert.equal(result.left, 0, "deleted from Recently Deleted, it is gone for good");
  assert.ok(!fs.existsSync(path.join(dir, ".trash", "Old.md")));
});

test("a folder is renamed in place, keeping its notes' pins, and never out of the notes directory or out of sight", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local note = s.create("Work", "Plan")
    s.set_pinned("Work/Plan.md", true)
    s.set_last("Work/Plan.md")
    s.create_folder("Home")
    local refused = {}
    for _, name in ipairs({ "../Out", ".hidden", "a/b", "..", "  ", "Home" }) do
      local to, why = s.rename_folder("Work", name)
      table.insert(refused, to == nil and why ~= nil)
    end
    local to = s.rename_folder("Work", " Projects ")
    io.write(vim.json.encode({ refused = refused, to = to, pinned = s.meta().pinned, last = s.meta().last }))
  `);
  assert.deepEqual(result.refused, [true, true, true, true, true, true], "out of the directory, hidden, nested, empty, or onto another folder");
  assert.equal(result.to, "Projects");
  assert.deepEqual(result.pinned, ["Projects/Plan.md"]);
  assert.equal(result.last, "Projects/Plan.md");
  assert.ok(fs.existsSync(path.join(dir, "Projects", "Plan.md")));
  assert.ok(!fs.existsSync(path.join(path.dirname(dir), "Out")), "nothing moved out");
});

test("search finds notes with every word, tags come from #words outside code, links find notes by title", () => {
  const { result } = lua(`
    local s = require("notes.store")
    local a = s.create("", "Trip")
    local f = io.open(a, "w"); f:write("# Trip\\n\\nPack the #travel bag and #Camping gear\\n\\n\`#notatag\`\\n\`\`\`\\n#nope\\n\`\`\`\\nissue#3 is not a tag\\n"); f:close()
    local b = s.create("Work", "Standup")
    local g = io.open(b, "w"); g:write("# Standup\\n\\nTalked about the #travel budget\\n"); g:close()
    io.write(vim.json.encode({
      both = vim.tbl_map(function(n) return n.title end, s.search("TRAVEL bag")),
      match = s.search("budget")[1].match,
      none = #s.search("submarine"),
      tags = s.tags(),
      tagged = vim.tbl_map(function(n) return n.title end, s.tagged("camping")),
      found = s.find_by_title("  standup ").rel,
      missing = s.find_by_title("Nope") == nil,
    }))
  `);
  assert.deepEqual(result.both, ["Trip"]);
  assert.equal(result.match, "Talked about the #travel budget");
  assert.equal(result.none, 0);
  assert.deepEqual(result.tags, [{ name: "Camping", count: 1 }, { name: "travel", count: 2 }]);
  assert.deepEqual(result.tagged, ["Trip"], "tags match without regard to case");
  assert.equal(result.found, "Work/Standup.md");
  assert.equal(result.missing, true);
});

test("each line's style is read from how it starts, and the first line is the title", () => {
  const { result } = lua(`
    local r = require("notes.render")
    local lines = { "Shopping", "", "## Today", "### Later", "- [ ] eggs", "  - [x] milk", "* one", "- two", "1. three", "> quote", "\`\`\`", "# not a heading", "\`\`\`", "| a | b |", "plain" }
    io.write(vim.json.encode(vim.tbl_map(function(k) return { k.kind, k.done, k.indent } end, r.kinds(lines))))
  `);
  assert.deepEqual(result.map((k) => k[0]), ["title", "body", "heading", "subheading", "check", "check", "bullet", "dash", "number", "quote", "fence", "code", "fence", "table", "body"]);
  assert.equal(result[4][1], false);
  assert.deepEqual(result[5].slice(1), [true, 2], "a ticked item, indented");
});

test("inline styles are found with their markup to hide, and code, snake_case and a#b are left alone", () => {
  const { result } = lua(`
    local r = require("notes.render")
    local line = "**b** *i* <u>u</u> ~~s~~ ==h== \`**c**\` [[Trip]] [site](https://x.co) https://a.b/c. #tag snake_case _it_ a#b"
    io.write(vim.json.encode(vim.tbl_map(function(s) return { s.hl, line:sub(s.s + 1, s.e), s.note or s.url or s.tag, vim.tbl_map(function(c) return line:sub(c[1] + 1, c[2]) end, s.conceal) } end, r.inline(line))))
  `);
  const by = Object.fromEntries(result.map(([hl, text, extra, hidden]) => [`${hl}:${text}`, { extra, hidden }]));
  assert.deepEqual(by["NotesBold:b"].hidden, ["**", "**"]);
  assert.deepEqual(by["NotesItalic:i"].hidden, ["*", "*"]);
  assert.deepEqual(by["NotesUnderline:u"].hidden, ["<u>", "</u>"]);
  assert.deepEqual(by["NotesStrike:s"].hidden, ["~~", "~~"]);
  assert.deepEqual(by["NotesHighlight:h"].hidden, ["==", "=="]);
  assert.deepEqual(by["NotesMono:**c**"].hidden, ["`", "`"], "nothing inside code is styled");
  assert.ok(!by["NotesBold:c"]);
  assert.equal(by["NotesNoteLink:Trip"].extra, "Trip");
  assert.deepEqual(by["NotesLink:site"], { extra: "https://x.co", hidden: ["[", "](https://x.co)"] });
  assert.equal(by["NotesLink:https://a.b/c"].extra, "https://a.b/c", "a full stop after a link isn't part of it");
  assert.equal(by["NotesTag:#tag"].extra, "tag");
  assert.ok(by["NotesItalic:it"], "_italic_ at word boundaries");
  assert.equal(result.filter(([hl]) => hl === "NotesItalic").length, 2, "snake_case isn't italic");
  assert.equal(result.filter(([hl]) => hl === "NotesTag").length, 1, "a#b isn't a tag");
});

test("paragraph styles replace a line's marker and toggle back to body; character styles wrap and unwrap", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_current_buf(buf)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "  - [ ] eggs", "milk", "bread" })
    local seen = {}
    local function snap() table.insert(seen, vim.api.nvim_buf_get_lines(buf, 0, -1, false)) end
    f.paragraph(buf, 1, 3, "number") snap()
    f.paragraph(buf, 1, 3, "number") snap()
    f.paragraph(buf, 2, 2, "heading") snap()
    f.paragraph(buf, 3, 3, "quote") snap()
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "say hello world" })
    vim.api.nvim_win_set_cursor(0, { 1, 5 })
    f.character(buf, "bold", false) snap()
    vim.api.nvim_win_set_cursor(0, { 1, 7 })
    f.character(buf, "bold", false) snap()
    f.wrap(buf, 1, 10, 15, "underline") snap()
    f.monospaced(buf, 1, 1) snap()
    f.monospaced(buf, 2, 2) snap()
    io.write(vim.json.encode(seen))
  `);
  assert.deepEqual(result[0], ["  1. eggs", "2. milk", "3. bread"], "numbered from 1, indent kept");
  assert.deepEqual(result[1], ["  eggs", "milk", "bread"], "the same style again is body text");
  assert.deepEqual(result[2][1], "## milk");
  assert.deepEqual(result[3][2], "> bread");
  assert.equal(result[4][0], "say **hello** world", "the word under the cursor");
  assert.equal(result[5][0], "say hello world", "and off again");
  assert.equal(result[6][0], "say hello <u>world</u>");
  assert.deepEqual(result[7], ["```", "say hello <u>world</u>", "```"]);
  assert.deepEqual(result[8], ["say hello <u>world</u>"], "monospaced again takes the block out");
});

test("a character style works as in Notes: on a word, off in text that has it, and on what you type next anywhere else", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_current_buf(buf)
    vim.o.virtualedit = "onemore"
    local seen = {}
    local function snap(label) table.insert(seen, { label, vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1], vim.api.nvim_win_get_cursor(0)[2], f.pending and f.pending.col or vim.NIL }) end
    local function at(col, text) vim.api.nvim_buf_set_lines(buf, 0, -1, false, { text }) vim.api.nvim_win_set_cursor(0, { 1, col }) f.pending = nil end
    at(6, "alpha beta gamma") f.character(buf, "bold") snap("word")
    at(8, "alpha **beta** gamma") f.character(buf, "bold") snap("off")
    at(7, "alpha **two words** x") f.character(buf, "bold") snap("off, the whole run")
    at(8, "alpha **beta** gamma") f.character(buf, "italic") snap("italic on bold")
    at(5, "alpha beta") f.character(buf, "strike") snap("on a space")
    at(0, "") f.character(buf, "underline") snap("empty line")
    f.drop_empty(buf) snap("left empty")
    at(4, "type") f.character(buf, "highlight", false, 4) snap("just typed")
    at(2, "type") f.character(buf, "code", false, 2) snap("typing inside a word")
    io.write(vim.json.encode(seen))
  `);
  const [word, off, run, both, space, empty, dropped, typed, inside] = result;
  assert.deepEqual(word.slice(1, 3), ["alpha **beta** gamma", 8]);
  assert.deepEqual(off.slice(1, 3), ["alpha beta gamma", 6], "on bold text, Bold takes it off, the cursor staying on the word");
  assert.equal(run[1], "alpha two words x", "for the whole run, not a word of it");
  assert.equal(both[1], "alpha ***beta*** gamma", "italic on a bold word adds to it");
  assert.deepEqual(space.slice(1), ["alpha ~~~~beta", 8, 6], "markers after the space, the cursor between them");
  assert.deepEqual(empty.slice(1), ["<u></u>", 3, 0]);
  assert.deepEqual(dropped.slice(1, 2), [""], "nothing typed in them: they go");
  assert.deepEqual(typed.slice(1), ["type====", 6, 4], "right after a word you typed, it is for what you type next");
  assert.deepEqual(inside.slice(1, 3), ["`type`", 3], "inside a word, the word, the cursor where it was in it");
});

test("typing starts after a line's marker: a new checklist item's cursor, and i, a and I on one", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_current_buf(buf)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "", "milk" })
    f.paragraph(buf, 1, 1, "check")
    local empty = vim.api.nvim_win_get_cursor(0)
    vim.api.nvim_win_set_cursor(0, { 2, 0 })
    f.paragraph(buf, 2, 2, "check")
    local with_text = vim.api.nvim_win_get_cursor(0)
    local p = f.insert_point
    io.write(vim.json.encode({
      lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false), empty = empty, with_text = with_text,
      a_on_box = p("- [ ] milk", 0, "a"), i_on_box = p("- [ ] milk", 2, "i"), I = p("  * milk", 5, "I"),
      a_in_text = p("- [ ] milk", 7, "a") == nil, plain = p("milk", 0, "I") == nil, heading = p("## Plan", 0, "a"),
    }))
  `);
  assert.deepEqual(result.lines, ["- [ ] ", "- [ ] milk"]);
  assert.deepEqual(result.empty, [1, 5], "on the item's last character, so a starts typing at its end");
  assert.deepEqual(result.with_text, [2, 6], "on its text");
  assert.equal(result.a_on_box, 6, "a on the ☐ types after it");
  assert.equal(result.i_on_box, 6);
  assert.equal(result.I, 4, "I goes past the marker, not to the bullet");
  assert.equal(result.a_in_text, true, "in the text, a is a as always");
  assert.equal(result.plain, true, "and a line without a marker is left alone");
  assert.equal(result.heading, 3);
});

test("checklists tick, move ticked items down, uncheck all and delete checked", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "# List", "- [ ] eggs", "- [ ] milk", "- [ ] bread", "", "after" })
    local seen = {}
    local function snap() table.insert(seen, vim.api.nvim_buf_get_lines(buf, 0, -1, false)) end
    local on_by_default = f.options.move_checked_to_bottom
    f.options.move_checked_to_bottom = false
    local not_item = f.toggle_check(buf, 1)
    f.toggle_check(buf, 2) snap()
    f.options.move_checked_to_bottom = true
    f.toggle_check(buf, 3) snap()
    f.uncheck_all(buf) snap()
    f.toggle_check(buf, 2)
    f.delete_checked(buf) snap()
    io.write(vim.json.encode({ not_item = not_item, seen = seen, on_by_default = on_by_default }))
  `);
  assert.equal(result.on_by_default, true, "a list leads with what is left unless turned off");
  assert.equal(result.not_item, false);
  assert.deepEqual(result.seen[0].slice(1, 4), ["- [x] eggs", "- [ ] milk", "- [ ] bread"]);
  assert.deepEqual(result.seen[1].slice(1, 5), ["- [ ] bread", "", "- [x] eggs", "- [x] milk"], "ticked items under the rest, in order, a blank line between");
  assert.deepEqual(result.seen[2].slice(1, 5), ["- [ ] bread", "- [ ] eggs", "- [ ] milk", ""], "the gap goes with the ticks");
  assert.deepEqual(result.seen[3], ["# List", "- [ ] eggs", "- [ ] milk", "", "after"]);
});

test("a ticked item moves down with its details and sub-items, and sub-items sort under their item", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, {
      "# Trip",
      "- [ ] book flights",
      "  compare two airlines",
      "  - [ ] outbound",
      "  - [ ] return",
      "- [ ] pack",
      "",
      "after",
    })
    local seen = {}
    local function snap() table.insert(seen, vim.api.nvim_buf_get_lines(buf, 0, -1, false)) end
    f.toggle_check(buf, 4) snap()
    f.toggle_check(buf, 2) snap()
    io.write(vim.json.encode({ seen = seen }))
  `);
  assert.deepEqual(result.seen[0], ["# Trip", "- [ ] book flights", "  compare two airlines", "  - [ ] return", "  - [x] outbound", "- [ ] pack", "", "after"], "a sub-item sorts under its item, its details first");
  assert.deepEqual(result.seen[1], ["# Trip", "- [ ] pack", "", "- [x] book flights", "  compare two airlines", "  - [ ] return", "  - [x] outbound", "", "after"], "an item moves with everything under it");
});

test("the gap before ticked items keeps the list one list, and a ticked item's lines are struck through", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local render = require("notes.render")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "# List", "- [ ] a", "- [ ] b", "", "- [x] c", "  why c", "  - more", "", "after" })
    f.toggle_check(buf, 2)
    local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    render.draw(buf)
    local struck = {}
    for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, render.ns, 0, -1, { details = true })) do
      if m[4].hl_group == "NotesCheckedStrike" then struck[m[2] + 1] = true end
    end
    io.write(vim.json.encode({ lines = lines, struck = vim.tbl_keys(struck) }))
  `);
  assert.deepEqual(result.lines, ["# List", "- [ ] b", "", "- [x] a", "- [x] c", "  why c", "  - more", "", "after"], "ticked across the gap joins the ticked half");
  assert.deepEqual(result.struck.sort((x, y) => x - y), [4, 5, 6, 7], "each ticked item and the lines under it, not the gap or what follows");
});

test("Enter carries a list on and ends it on an empty item; Tab indents an item and renumbers", () => {
  const { result } = lua(`
    local f = require("notes.format")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_current_buf(buf)
    -- As in Insert mode, the cursor can be just past a line's end.
    vim.o.virtualedit = "onemore"
    -- Your config's indent, not Neovim's default of 8.
    vim.o.shiftwidth = 2
    local seen = {}
    local function snap() table.insert(seen, { lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false), cursor = vim.api.nvim_win_get_cursor(0) }) end
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "- [x] eggs" })
    vim.api.nvim_win_set_cursor(0, { 1, 10 })
    local carried = f.enter(buf) snap()
    f.enter(buf) snap()
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "1. one", "2. two" })
    vim.api.nvim_win_set_cursor(0, { 1, 5 })
    f.enter(buf) snap()
    vim.api.nvim_win_set_cursor(0, { 2, 3 })
    f.tab(buf, 1) snap()
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "plain" })
    local plain = f.enter(buf)
    io.write(vim.json.encode({ carried = carried, plain = plain, seen = seen }))
  `);
  assert.equal(result.carried, true);
  assert.deepEqual(result.seen[0].lines, ["- [x] eggs", "- [ ] "], "a new item, unticked");
  assert.deepEqual(result.seen[0].cursor, [2, 6]);
  assert.deepEqual(result.seen[1].lines, ["- [x] eggs", ""], "Enter on an empty item ends the list");
  assert.deepEqual(result.seen[2].lines, ["1. on", "2. e", "3. two"], "split at the cursor, and the rest renumbered");
  assert.deepEqual(result.seen[3].lines, ["1. on", "  1. e", "2. two"], "indented, it starts its own numbering");
  assert.equal(result.plain, false, "not a list: Enter does what it always does");
});

test("tables line up as they show, and Tab moves between cells, adding a row past the last", () => {
  const { result } = lua(`
    local t = require("notes.table")
    local buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_set_current_buf(buf)
    local aligned = t.aligned({ "| Item | Qty |", "|:-|-:|", "| **Eggs** | 12 |", "| a \\\\| b | 1 |" })
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "| A | B |", "| --- | --- |", "| x | y |" })
    vim.api.nvim_win_set_cursor(0, { 1, 2 })
    local cells = {}
    for _ = 1, 4 do
      t.jump(buf, 1)
      table.insert(cells, vim.api.nvim_win_get_cursor(0))
    end
    local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    vim.api.nvim_win_set_cursor(0, { 1, 2 })
    t.add_column(buf)
    local widened = vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1]
    vim.api.nvim_win_set_cursor(0, { 1, 8 })
    t.delete_column(buf)
    io.write(vim.json.encode({ aligned = aligned, cells = cells, lines = lines, widened = widened, narrowed = vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1] }))
  `);
  assert.deepEqual(result.aligned, ["| Item   | Qty |", "| :----- | --: |", "| **Eggs**   | 12  |", "| a \\| b | 1   |"], "bold markup doesn't count toward a cell's width");
  assert.deepEqual(result.cells, [[1, 8], [3, 2], [3, 8], [4, 2]], "next cell, over the separator, then a new row");
  assert.equal(result.lines.length, 4);
  assert.equal(result.widened, "| A   |     | B   |");
  assert.equal(result.narrowed, "| A   | B   |");
});

test(":Notes opens the sidebar and the newest note, a welcome note at first; a new note is named once it has a title", () => {
  const { result, dir } = lua(`
    vim.cmd("Notes")
    local n = require("notes")
    local s = require("notes.sidebar")
    local first = { wins = #vim.api.nvim_tabpage_list_wins(0), note = vim.fs.basename(n.current_path()), sidebar = vim.api.nvim_buf_get_lines(s.buf, 0, 12, false) }
    s.new_note()
    local buf = vim.api.nvim_get_current_buf()
    local title_at = vim.api.nvim_win_get_cursor(0)
    vim.cmd("stopinsert")
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "# Groceries", "", "- [ ] eggs #food" })
    n.save(buf)
    local after = vim.fs.basename(vim.api.nvim_buf_get_name(buf))
    local rendered = #vim.api.nvim_buf_get_extmarks(buf, require("notes.render").ns, 0, -1, {})
    local menu_key = vim.fn.maparg("<C-p>", "n", false, true)
    local insert_key = vim.fn.maparg("<C-p>", "i", false, true)
    -- What Enter and a click do: a click ticks only on the box itself.
    local on_text = n.act(buf, 3, 9, true)
    local on_box = n.act(buf, 3, 2, true)
    local ticked = vim.api.nvim_buf_get_lines(buf, 2, 3, false)[1]
    local folded = n.act(buf, 1, 0, false)
    local closed = vim.fn.foldclosed(1)
    s.show({ tag = "food" })
    local tag_view = vim.tbl_filter(function(l) return l:match("Groceries") end, vim.api.nvim_buf_get_lines(s.buf, 0, -1, false))
    io.write(vim.json.encode({ first = first, title_at = title_at, menu_key = { buffer = menu_key.buffer, desc = menu_key.desc }, insert_key = { buffer = insert_key.buffer, desc = insert_key.desc }, after = after, rendered = rendered, on_text = on_text, on_box = on_box, ticked = ticked, folded = folded, closed = closed, tag_view = tag_view, files = vim.fn.readdir(vim.env.NOTES_DIR) }))
  `);
  assert.equal(result.first.wins, 2);
  assert.equal(result.first.note, "Welcome to Notes.md");
  assert.ok(result.first.sidebar.some((line) => /All Notes\s+1$/.test(line)));
  assert.deepEqual(result.title_at, [1, 2], "a new note starts with its title to type, after the #");
  assert.equal(result.after, "Groceries.md");
  assert.deepEqual(result.menu_key, { buffer: 1, desc: "Format menu" }, "Ctrl-P opens the Format menu, in a note only");
  assert.deepEqual(result.insert_key, { buffer: 1, desc: "Format menu" }, "in Insert mode too, rather than vim's word completion");
  assert.ok(result.rendered > 0);
  assert.equal(result.on_text, false);
  assert.equal(result.on_box, true);
  assert.equal(result.ticked, "- [x] eggs #food");
  assert.equal(result.folded, true);
  assert.equal(result.closed, 1, "Enter on the title folds its section");
  assert.equal(result.tag_view.length, 1, "a tag shows the notes that have it");
  assert.deepEqual(result.files.filter((f) => f.endsWith(".md")).sort(), ["Groceries.md", "Welcome to Notes.md"]);
  assert.ok(!fs.existsSync(path.join(dir, "New Note.md")));
});

test("the sidebar grows back to its width when the screen does, and is never narrower than its minimum", () => {
  const { result } = lua(`
    vim.o.columns = 200
    vim.cmd("Notes")
    local s = require("notes.sidebar")
    local width = function() return vim.api.nvim_win_get_width(s.window()) end
    local wide = width()
    vim.o.columns = 30
    vim.api.nvim_win_set_width(s.window(), 8)
    vim.o.columns = 70
    vim.api.nvim_exec_autocmds("VimResized", {})
    local narrow = width()
    vim.o.columns = 200
    vim.api.nvim_exec_autocmds("VimResized", {})
    io.write(vim.json.encode({ wide = wide, narrow = narrow, back = width(), full = s.WIDTH, min = require("notes").MIN_WIDTH }))
  `);
  assert.equal(result.wide, result.full);
  assert.equal(result.narrow, result.min, "squeezed, it keeps its minimum");
  assert.equal(result.back, result.full, "and grows back to full width");
});

test("Notes opens the note you had open last, through renames, and the newest once that one is deleted", () => {
  const { result: first, dir } = lua(`
    local s = require("notes.store")
    s.create("", "Old")
    vim.uv.fs_utime(s.absolute("Old.md"), os.time() - 600, os.time() - 600)
    s.create("", "Newer")
    s.set_pinned("Newer.md", true)
    vim.cmd("Notes")
    local n = require("notes")
    local opened = vim.fs.basename(n.current_path())
    -- Read, not changed, then renamed by its title.
    n.open(s.absolute("Old.md"))
    local buf = vim.api.nvim_get_current_buf()
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { "# Old, renamed" })
    n.save(buf)
    io.write(vim.json.encode({ opened = opened, last = s.meta().last }))
  `);
  assert.equal(first.opened, "Newer.md", "at first, the top of the list: the pinned note");
  assert.equal(first.last, "Old, renamed.md", "the one open last, under its new name");
  const { result: again } = lua(`
    vim.cmd("Notes")
    local n = require("notes")
    local reopened = vim.fs.basename(n.current_path())
    local s = require("notes.store")
    s.delete(s.absolute("Old, renamed.md"))
    io.write(vim.json.encode({ reopened = reopened, last = s.meta().last or vim.NIL, fallback = vim.fs.basename(s.last() or "none") }))
  `, dir);
  assert.equal(again.reopened, "Old, renamed.md", "started again, it opens where you left off, though another note is pinned and newer");
  assert.equal(again.last, null, "deleted, it is no longer the one to open");
  assert.equal(again.fallback, "none");
});

test("two edits of a note merge line by line: next to each other both apply, on the same line both versions are kept, yours first", () => {
  const { result } = lua(`
    local merge = require("notes.sync").merge
    local base = "# List\\n- [ ] one\\n- [ ] two\\n- [ ] three\\n"
    local adjacent = { merge(base, "# List\\n- [ ] one, typed\\n- [ ] two\\n- [ ] three\\n", "# List\\n- [ ] one\\n- [ ] two (ABC-1)\\n- [ ] three\\n") }
    local same = { merge(base, "# List\\n- [ ] one\\n- [ ] two, mine\\n- [ ] three\\n", "# List\\n- [ ] one\\n- [ ] two (ABC-1)\\n- [ ] three\\n") }
    local alike = { merge(base, "# List\\n- [x] one\\n- [ ] two\\n- [ ] three\\n", "# List\\n- [x] one\\n- [ ] two\\n- [ ] three\\n") }
    local added = { merge(base, base .. "- [ ] mine\\n", base .. "- [ ] theirs\\n") }
    local removed = { merge(base, "# List\\n- [ ] two\\n- [ ] three\\n", "# List\\n- [ ] one\\n- [ ] two\\n- [ ] three\\n- [ ] four\\n") }
    io.write(vim.json.encode({ adjacent = adjacent, same = same, alike = alike, added = added, removed = removed }))
  `);
  assert.deepEqual(result.adjacent, ["# List\n- [ ] one, typed\n- [ ] two (ABC-1)\n- [ ] three\n", 0]);
  assert.deepEqual(result.same, ["# List\n- [ ] one\n- [ ] two, mine\n- [ ] two (ABC-1)\n- [ ] three\n", 1]);
  assert.deepEqual(result.alike, ["# List\n- [x] one\n- [ ] two\n- [ ] three\n", 0], "the same change made on both sides appears once");
  assert.deepEqual(result.added, ["# List\n- [ ] one\n- [ ] two\n- [ ] three\n- [ ] mine\n- [ ] theirs\n", 1]);
  assert.deepEqual(result.removed, ["# List\n- [ ] two\n- [ ] three\n- [ ] four\n", 0]);
});

test("an item ticked from outside moves under the ones left to do, and the note is saved that way", () => {
  const { result } = lua(`
    local s = require("notes.store")
    local path = s.create("", "Errands")
    local f = io.open(path, "w"); f:write("# Errands\\n\\n- [ ] milk\\n- [ ] eggs\\n- [ ] bread\\n"); f:close()
    vim.cmd("Notes")
    local n = require("notes")
    n.open(path)
    local buf = vim.api.nvim_get_current_buf()
    local function lines() return table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\\n") end
    local g = io.open(path, "w"); g:write("# Errands\\n\\n- [x] milk\\n- [ ] eggs\\n- [ ] bread\\n"); g:close()
    local moved = vim.wait(3000, function() return lines():find("bread\\n\\n%- %[x%] milk") ~= nil end, 20)
    local saved = vim.wait(3000, function() return io.open(path):read("*a"):find("bread\\n\\n%- %[x%] milk") ~= nil end, 20)
    io.write(vim.json.encode({ moved = moved, saved = saved, text = io.open(path):read("*a") }))
  `);
  assert.equal(result.moved, true, "ticked from outside, it moved down in the open note");
  assert.equal(result.saved, true, `and the file says so: ${result.text}`);
});

test("a note changed from outside while it is open shows the change, and a save keeps both it and what you typed", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local path = s.create("", "Plan")
    local f = io.open(path, "w"); f:write("# Plan\\n\\n- [ ] one\\n- [ ] two\\n- [ ] three\\n"); f:close()
    vim.cmd("Notes")
    local n = require("notes")
    n.open(path)
    local buf = vim.api.nvim_get_current_buf()
    vim.api.nvim_win_set_cursor(0, { 5, 4 })
    local function outside(text) local g = io.open(path, "w"); g:write(text); g:close() end
    local function lines() return table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\\n") end

    -- Nothing unsaved: the change shows up on its own, the cursor where it was.
    outside("# Plan\\n\\n- [ ] one (ABC-1)\\n- [ ] two\\n- [ ] three\\n")
    local shown = vim.wait(3000, function() return lines():find("ABC%-1") ~= nil end, 20)
    local after_shown = { lines = lines(), modified = vim.bo[buf].modified, cursor = vim.api.nvim_win_get_cursor(0) }

    -- Typing not yet saved, and a change outside on the next line: the save
    -- has both. Nothing waits here, so it is the save that brings it in.
    vim.api.nvim_buf_set_lines(buf, 3, 4, false, { "- [ ] two, typed" })
    outside("# Plan\\n\\n- [ ] one (ABC-1)\\n- [ ] two\\n- [ ] three (ABC-2)\\n")
    n.save(buf)
    local saved = io.open(path):read("*a")

    -- The app's own save doesn't come back as a change.
    local tick = vim.b[buf].changedtick
    vim.wait(400)
    local own = vim.b[buf].changedtick == tick

    -- Deleted outside: what you type isn't written back under the old name.
    os.remove(path)
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { "# Plan, still" })
    n.save(buf)
    io.write(vim.json.encode({ shown = shown, after_shown = after_shown, saved = saved, own = own, recreated = vim.uv.fs_stat(path) ~= nil, kept = lines():find("Plan, still") ~= nil }))
  `);
  assert.equal(result.shown, true, "the open note took the outside change without a save or a keypress");
  assert.equal(result.after_shown.lines, "# Plan\n\n- [ ] one (ABC-1)\n- [ ] two\n- [ ] three");
  assert.equal(result.after_shown.modified, false, "and has nothing unsaved because of it");
  assert.deepEqual(result.after_shown.cursor, [5, 4]);
  assert.match(result.saved, /^# Plan\n\n- \[ \] one \(ABC-1\)\n- \[ \] two, typed\n- \[ \] three \(ABC-2\)\n\n<!-- note-id: n-[0-9a-f]{12} -->\n$/, "with its ID, which the outside change left out, put back last");
  assert.equal(result.own, true);
  assert.equal(result.recreated, false);
  assert.equal(result.kept, true, "the text stays in the note window");
  assert.ok(!fs.existsSync(path.join(dir, "Plan, still.md")));
});

test("the note window numbers its lines relative to the cursor, and the sidebar doesn't; the option turns them off", () => {
  const { result } = lua(`
    vim.cmd("Notes")
    local n = require("notes")
    local note = { number = vim.wo[n.note_win].number, relative = vim.wo[n.note_win].relativenumber }
    local side = require("notes.sidebar").window()
    local sidebar = { number = vim.wo[side].number, relative = vim.wo[side].relativenumber }
    n.setup({ relative_line_numbers = false })
    n.open(n.current_path())
    io.write(vim.json.encode({ note = note, sidebar = sidebar, off = { number = vim.wo[n.note_win].number, relative = vim.wo[n.note_win].relativenumber } }))
  `);
  assert.deepEqual(result.note, { number: true, relative: true });
  assert.deepEqual(result.sidebar, { number: false, relative: false });
  assert.deepEqual(result.off, { number: false, relative: false });
});

test("a note opened from outside, as M-e opens a tab's, puts the sidebar's cursor on it, its folder's view left for All Notes when that doesn't list it", () => {
  const { result } = lua(`
    local s = require("notes.store")
    s.create("", "Loose")
    s.create_folder("Work")
    local work = s.create("Work", "Plan")
    vim.cmd("Notes")
    local n = require("notes")
    local sb = require("notes.sidebar")
    sb.show({ folder = "" })
    local before = vim.api.nvim_win_get_cursor(sb.window())[1]
    n.open(work, { stay = true })
    vim.wait(50)
    local at = vim.api.nvim_win_get_cursor(sb.window())[1]
    local seen = { moved = at ~= before, on = sb.state.items[at] and sb.state.items[at].note == vim.fs.normalize(work), all = sb.state.folder == nil }
    -- The open note deleted from the sidebar: the cursor on the note opened in its place.
    n.open(s.absolute("Loose.md"), { stay = true })
    vim.wait(50)
    vim.api.nvim_set_current_win(sb.window())
    sb.delete()
    seen.replaced = vim.fs.normalize(n.current_path()) ~= vim.fs.normalize(s.absolute("Loose.md"))
    vim.wait(50)
    local row = vim.api.nvim_win_get_cursor(sb.window())[1]
    seen.after_delete = sb.state.items[row] and sb.state.items[row].note == vim.fs.normalize(n.current_path())
    io.write(vim.json.encode(seen))
  `);
  assert.deepEqual(result, { moved: true, on: true, all: true, replaced: true, after_delete: true });
});

test("the sidebar is put away and shown again on asking, and NOTES_SIDEBAR=off starts the app without it", () => {
  const code = (start) => `
    local s = require("notes.store")
    local first = s.create("", "First")
    local file = assert(io.open(first, "w"))
    file:write("# First\\n#todo\\n")
    file:close()
    ${start}
    vim.cmd("Notes")
    local n = require("notes")
    local sb = require("notes.sidebar")
    local seen = { started = sb.window() ~= nil }
    n.show_sidebar(false)
    seen.hidden = sb.window() == nil
    seen.in_note = vim.api.nvim_get_current_win() == n.note_win
    n.show_sidebar(false)
    seen.windows = #vim.api.nvim_tabpage_list_wins(0)
    n.show_sidebar(true)
    seen.back = sb.window() ~= nil
    -- Ctrl-B, from a note, typing in it or not, and from the list.
    n.show_sidebar(false)
    vim.api.nvim_set_current_win(n.note_win)
    vim.api.nvim_feedkeys(vim.keycode("<C-b>"), "x", false)
    seen.key_shown = sb.window() ~= nil
    vim.api.nvim_set_current_win(sb.window())
    vim.api.nvim_feedkeys(vim.keycode("<C-b>"), "x", false)
    seen.key_hidden = sb.window() == nil and vim.api.nvim_get_current_win() == n.note_win
    vim.api.nvim_feedkeys(vim.keycode("i<C-b>"), "x", false)
    seen.key_typing = sb.window() ~= nil
    vim.cmd("stopinsert")
    -- A search that leaves out the open note is still there when it comes back.
    sb.show({ query = "nothing like it" })
    vim.api.nvim_set_current_win(sb.window())
    vim.api.nvim_feedkeys(vim.keycode("<C-b>"), "x", false)
    vim.api.nvim_feedkeys(vim.keycode("<C-b>"), "x", false)
    seen.view_kept = sb.window() ~= nil and sb.state.query == "nothing like it"
    sb.show({})
    n.show_sidebar(false)
    require("notes.links").follow(vim.api.nvim_win_get_buf(n.note_win), 2, 1)
    seen.tag = sb.window() ~= nil and sb.state.tag == "todo"
    io.write(vim.json.encode(seen))
  `;
  assert.deepEqual(lua(code("")).result, { started: true, hidden: true, in_note: true, windows: 1, back: true, key_shown: true, key_hidden: true, key_typing: true, view_kept: true, tag: true }, "and a #tag followed brings it back with the tag's notes");
  assert.equal(lua(code('vim.env.NOTES_SIDEBAR = "off"')).result.started, false, "started beside a tab, without it");
});

test("completion and Copilot are off in every note, opened at the start or from the sidebar, before its type is set", () => {
  const { result } = lua(`
    -- nvim-cmp isn't loaded in the notes app at all, so its Ctrl-P never
    -- takes the Format menu's: its spec in the config is off with g:notes_app.
    vim.g.notes_app = 1
    local cmp_spec = vim.tbl_filter(function(spec) return spec[1] == "hrsh7th/nvim-cmp" end, dofile("${path.join(__dirname, "../nvim/lua/plugins/lsp.lua")}"))[1]
    -- File types are detected, as in any real config (-u NONE leaves it off).
    vim.cmd("filetype on")
    -- What a plugin attaching on FileType, as Copilot does, would see.
    local at_filetype = {}
    vim.api.nvim_create_autocmd("FileType", { pattern = "markdown", callback = function(ev) table.insert(at_filetype, vim.b[ev.buf].copilot_enabled == false) end })
    local s = require("notes.store")
    s.create("", "First")
    vim.uv.fs_utime(s.absolute("First.md"), os.time() - 60, os.time() - 60)
    s.create("", "Second")
    vim.cmd("Notes")
    local n = require("notes")
    local sb = require("notes.sidebar")
    local at_start = vim.api.nvim_win_get_buf(n.note_win)
    -- Opened from the sidebar, with the sidebar the current window.
    vim.api.nvim_set_current_win(sb.window())
    n.open(s.absolute("First.md"), { stay = true })
    local from_sidebar = vim.api.nvim_win_get_buf(n.note_win)
    io.write(vim.json.encode({
      cmp_loaded = cmp_spec.cond,
      format_menu = { vim.api.nvim_buf_call(at_start, function() return vim.fn.maparg("<C-p>", "i", false, true).desc end), vim.api.nvim_buf_call(from_sidebar, function() return vim.fn.maparg("<C-p>", "i", false, true).desc end) },
      copilot = vim.b[from_sidebar].copilot_enabled,
      at_filetype = at_filetype,
      undofile = vim.bo[from_sidebar].undofile,
    }))
  `);
  assert.deepEqual(result, { cmp_loaded: false, format_menu: ["Format menu", "Format menu"], copilot: false, at_filetype: [true, true], undofile: false });
});

test("Ctrl-F lists every note for fzf, newest first, and opens the one picked", () => {
  // A stand-in fzf that keeps what it was given, and picks the second line.
  const bin = fs.mkdtempSync(path.join(tmp, "fzf-bin-"));
  const given = path.join(bin, "given.txt");
  fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\necho "$@" > '${given}.args'\ntee '${given}' | sed -n 2p\n`, { mode: 0o755 });
  const { result } = lua(`
    vim.env.PATH = "${bin}:" .. vim.env.PATH
    local s = require("notes.store")
    s.create("Work", "Standup")
    vim.uv.fs_utime(s.absolute("Work/Standup.md"), os.time() - 60, os.time() - 60)
    s.create("", "Groceries")
    s.delete(s.create("", "Gone"))
    vim.cmd("Notes")
    local n = require("notes")
    local key = vim.fn.maparg("<C-f>", "n", false, true)
    require("notes.finder").open()
    vim.wait(3000, function() return n.current_path() and n.current_path():match("Standup") ~= nil end)
    io.write(vim.json.encode({ opened = vim.fs.basename(n.current_path()), key = key.desc, wins = #vim.api.nvim_tabpage_list_wins(0) }))
  `);
  const lines = fs.readFileSync(given, "utf8").replace(/\n$/, "").split("\n");
  assert.equal(lines.length, 2, "Recently Deleted isn't listed");
  assert.match(lines[0], /^Groceries\.md\t  Groceries\s+Notes\s+$/);
  assert.match(lines[1], /^Work\/Standup\.md\t  Standup\s+Work\s+$/);
  assert.match(fs.readFileSync(`${given}.args`, "utf8"), /--with-nth=2 .*--preview=cat \{1\}/);
  assert.deepEqual(result, { opened: "Standup.md", key: "Find a note", wins: 2 }, "the picked note opens, and the finder's window is gone");
});

test("Ctrl-Shift-P lists every Notes command with its key, runs the one picked where it was opened, and lists the last picked first", () => {
  // A stand-in fzf that keeps what it was given, and picks the line naming $PICK.
  const bin = fs.mkdtempSync(path.join(tmp, "palette-bin-"));
  const given = path.join(bin, "given.txt");
  fs.writeFileSync(path.join(bin, "fzf"), `#!/bin/sh\ntee '${given}' | grep -m1 -F "$PICK"\n`, { mode: 0o755 });
  const { result } = lua(`
    vim.env.PATH = "${bin}:" .. vim.env.PATH
    local s = require("notes.store")
    local path = s.create("", "Plan")
    local file = assert(io.open(path, "w"))
    file:write("# Plan\\nShip it\\n")
    file:close()
    vim.cmd("Notes")
    local n = require("notes")
    local sb = require("notes.sidebar")
    local seen = {}
    local function pick(name)
      vim.env.PICK = name
      local before = vim.api.nvim_get_current_win()
      vim.api.nvim_feedkeys(vim.keycode("<C-S-p>"), "x", false)
      vim.wait(3000, function() return vim.api.nvim_get_current_win() == before or vim.api.nvim_get_current_win() == n.note_win end)
      vim.wait(200)
    end
    -- From a note, a Format menu command on the line the cursor is on.
    vim.api.nvim_set_current_win(n.note_win)
    vim.api.nvim_win_set_cursor(n.note_win, { 2, 0 })
    pick("Paragraph: Heading")
    vim.cmd("stopinsert")
    seen.heading = vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(n.note_win), 1, 2, false)[1]
    -- The list's toggle, from the note.
    pick("Show or put away the list")
    seen.list_hidden = sb.window() == nil
    -- From the list, where the Format menu isn't offered.
    n.toggle_sidebar()
    vim.api.nvim_set_current_win(sb.window())
    pick("Show or put away the list")
    seen.list_hidden_from_list = sb.window() == nil
    seen.lines = vim.fn.readfile("${given}")
    io.write(vim.json.encode(seen))
  `);
  assert.equal(result.heading, "## Ship it", "the Format menu's Heading, on the cursor's line");
  assert.equal(result.list_hidden, true, "the list put away from a note");
  assert.equal(result.list_hidden_from_list, true, "and from the list");
  const shown = result.lines.map((l) => l.split("\t")[1].replace(/\x1b\[[0-9;]*m/g, ""));
  assert.match(shown[0], /^Show or put away the list\s+Ctrl-B\s+#view$/, "the one picked last first, with its key and tag");
  assert.ok(shown.some((l) => /^Move Notes beside your work.*M-e/.test(l)), "Switchboard's commands for Notes");
  assert.ok(!shown.some((l) => /^Paragraph: Title/.test(l)), "no Format menu from the list");
});

test("only one Notes opens the notes at a time; a lock left by one that has gone is taken over, and quitting lets go", () => {
  const dir = fs.mkdtempSync(path.join(tmp, "notes-lock-"));
  // Held by a live process: this test's own.
  fs.writeFileSync(path.join(dir, ".notes.lock"), String(process.pid));
  const { result: refused } = lua(`
    vim.cmd("Notes")
    io.write(vim.json.encode({ wins = #vim.api.nvim_tabpage_list_wins(0), text = table.concat(vim.api.nvim_buf_get_lines(0, 0, -1, false), "\\n"), notes = #vim.fn.glob(vim.env.NOTES_DIR .. "/*.md", false, true) }))
  `, dir);
  assert.equal(refused.wins, 1, "no sidebar, no note");
  assert.match(refused.text, new RegExp(`Notes is open already, in another Neovim \\(process ${process.pid}\\)`));
  assert.equal(refused.notes, 0, "and not even the welcome note written");
  assert.equal(fs.readFileSync(path.join(dir, ".notes.lock"), "utf8"), String(process.pid), "the lock is left to its holder");

  // Left by a process that has gone: taken over, and let go on quitting.
  fs.writeFileSync(path.join(dir, ".notes.lock"), "999999");
  const { result: opened } = lua(`
    vim.cmd("Notes")
    local held = io.open(vim.env.NOTES_DIR .. "/.notes.lock"):read("*a")
    io.write(vim.json.encode({ wins = #vim.api.nvim_tabpage_list_wins(0), held = held == tostring(vim.uv.os_getpid()) }))
    vim.cmd("doautocmd VimLeavePre")
  `, dir);
  assert.deepEqual(opened, { wins: 2, held: true });
  assert.ok(!fs.existsSync(path.join(dir, ".notes.lock")), "gone once it quits");
  assert.ok(!fs.readdirSync(dir).some((name) => name.endsWith(".tmp")), "the pins were written whole, with nothing left beside them");
});

test("deleting the open note from the sidebar moves it to Recently Deleted and shows the next one", () => {
  const { result } = lua(`
    local s = require("notes.store")
    s.create("", "Keep")
    vim.uv.fs_utime(s.absolute("Keep.md"), os.time() - 60, os.time() - 60)
    s.create("", "Drop")
    vim.cmd("Notes")
    local n = require("notes")
    local sb = require("notes.sidebar")
    local opened = vim.fs.basename(n.current_path())
    vim.api.nvim_set_current_win(sb.window())
    for row, item in pairs(sb.state.items) do
      if item.note and item.note:match("Drop") then vim.api.nvim_win_set_cursor(0, { row, 0 }) break end
    end
    sb.delete()
    io.write(vim.json.encode({ opened = opened, now = vim.fs.basename(n.current_path()), trash = vim.tbl_map(function(x) return x.title end, s.notes(s.TRASH)) }))
  `);
  assert.equal(result.opened, "Drop.md", "the newest note opens first");
  assert.equal(result.now, "Keep.md");
  assert.deepEqual(result.trash, ["Drop"]);
});

test("bin/notes adds the plugin after your config loads, which lazy.nvim's reset would otherwise drop, and keeps Copilot out", () => {
  const script = fs.readFileSync(path.join(__dirname, "../bin/notes"), "utf8");
  assert.match(script, /exec nvim "\$\{listen\[@\]\}" --cmd "let g:notes_app = 1" -c/, "set before the config, so lazy.nvim sees it");
  assert.match(fs.readFileSync(path.join(__dirname, "../nvim/lua/plugins/copilot.lua"), "utf8"), /cond = not vim\.g\.notes_app/);
  assert.match(script, /-c "set rtp\^=\$here\/notes" -c "runtime plugin\/notes\.lua" -c "Notes"/);
  assert.doesNotMatch(script, /--cmd "set rtp/);
});

test("note-watch's watcher reports an item added to a note once its text settles, and the note going away", async () => {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "note-watch-"));
  const note = path.join(dir, "Plan.md");
  fs.writeFileSync(note, "# Plan\n\n- [ ] already here\n- [x] done\n");
  // Run through require, so the test knows when it has read the note: the
  // script reads the items already there before require returns. The times
  // leave room for a loaded machine: the item settles a second after the
  // last keystroke, and the test waits for what it reports rather than a set
  // time.
  const script = path.join(__dirname, "../claude/skills/note-watch/scripts/watch-note.js");
  const watcher = spawn(process.execPath, ["-e", `process.argv = [process.argv[0], ${JSON.stringify(script)}, ${JSON.stringify(note)}]; require(${JSON.stringify(script)}); console.error("ready")`], { env: { ...process.env, NOTE_WATCH_POLL_MS: "50", NOTE_WATCH_SETTLE_MS: "1000" } });
  let out = "";
  let err = "";
  watcher.stdout.on("data", (data) => (out += data));
  watcher.stderr.on("data", (data) => (err += data));
  const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
  try {
    for (const end = Date.now() + 10_000; !err.includes("ready") && Date.now() < end; ) await pause(20);
    // Half typed, then whole: only the whole one, once it has stopped changing.
    fs.writeFileSync(note, "# Plan\n\n- [ ] already here\n- [ ] \n- [ ] half\n- [x] done\n");
    await pause(120);
    fs.writeFileSync(note, "# Plan\n\n- [ ] already here\n- [ ] \n- [ ] half typed, now whole\n- [x] done\n");
    for (const end = Date.now() + 10_000; !out.includes("NEW") && Date.now() < end; ) await pause(20);
    fs.rmSync(note);
    await new Promise((resolve) => watcher.on("exit", resolve));
  } finally {
    watcher.kill();
    fs.rmSync(dir, { recursive: true, force: true });
  }
  assert.deepEqual(out.trim().split("\n").map((line) => line.split(":")[0] + (line.startsWith("NEW") ? line.slice(3) : "")), ["NEW: half typed, now whole", "GONE"]);
});


test("picking a style from the Format menu goes on to the note's own a, to type in it, and an action like ticking doesn't", () => {
  const { result } = lua(`
    vim.cmd("Notes")
    local n = require("notes")
    require("notes.sidebar").new_note()
    local buf = vim.api.nvim_get_current_buf()
    vim.cmd("stopinsert")
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "# Groceries", "", "eggs", "- [ ] milk" })
    -- What the menu types after a pick, without typing it: a headless Neovim
    -- left in Insert mode stops reading this script.
    local fed = {}
    local feedkeys = vim.api.nvim_feedkeys
    vim.api.nvim_feedkeys = function(keys, mode, escape)
      if keys == "a" then return table.insert(fed, { keys = keys, mode = mode }) end
      return feedkeys(keys, mode, escape)
    end
    vim.api.nvim_win_set_cursor(0, { 3, 0 })
    n.menu(buf, false)
    feedkeys("8", "x", false)
    vim.wait(50, function() return false end)
    local styled = { line = vim.api.nvim_get_current_line(), fed = vim.deepcopy(fed) }
    fed = {}
    vim.api.nvim_win_set_cursor(0, { 4, 0 })
    n.menu(buf, false)
    feedkeys("c", "x", false)
    vim.wait(50, function() return false end)
    io.write(vim.json.encode({ styled = styled, ticked = { line = vim.api.nvim_buf_get_lines(buf, 3, 4, false)[1], fed = fed } }))
  `);
  assert.equal(result.styled.line, "* eggs");
  assert.deepEqual(result.styled.fed, [{ keys: "a", mode: "m" }], "a bulleted list is picked to write in, through the note's mapped a");
  assert.equal(result.ticked.line, "- [x] milk");
  assert.deepEqual(result.ticked.fed, [], "ticking a box is done, not written in");
});

test("an open note moved to another folder keeps its time, which says when its text last changed", () => {
  const { result } = lua(`
    local s = require("notes.store")
    s.create_folder("Work")
    local plan = s.create("", "Plan")
    vim.uv.fs_utime(plan, 1767225600, 1767225600)
    vim.cmd("Notes")
    local n = require("notes")
    n.open(plan, { stay = true })
    local to = s.move(plan, "Work")
    n.moved(plan, to)
    io.write(vim.json.encode({ at = vim.uv.fs_stat(to).mtime.sec, open = vim.fs.normalize(n.current_path()) == vim.fs.normalize(to) }))
  `);
  assert.deepEqual(result, { at: 1767225600, open: true });
});

test("squashing notes merges them with Claude into a new note, in the first one's folder, the originals to Recently Deleted", (t) => {
  const bin = fs.mkdtempSync(path.join(tmp, "claude-bin-"));
  const asked = path.join(bin, "asked");
  // A stand-in claude -p: keeps what it was given, answers a merged note.
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\nprintf '%s\\n' "$@" > '${asked}'\ncat >> '${asked}'\nprintf '# Groceries and errands\\n\\n- [ ] milk\\n- [ ] post office\\n'\n`, { mode: 0o755 });
  const saved = { PATH: process.env.PATH, TMUX: process.env.TMUX };
  process.env.PATH = `${bin}:${saved.PATH}`;
  delete process.env.TMUX;
  t.after(() => { process.env.PATH = saved.PATH; });
  const { result, dir } = lua(`
    local s = require("notes.store")
    local a = s.create("", "Groceries")
    local f = io.open(a, "w"); f:write("# Groceries\\n\\n- [ ] milk\\n\\n<!-- note-id: n-aaaaaaaaaaaa -->\\n"); f:close()
    local b = s.create("", "Errands")
    local g = io.open(b, "w"); g:write("# Errands\\n\\n- [ ] post office\\n"); g:close()
    vim.uv.fs_utime(a, 1767225600, 1767225600)
    local made
    require("notes.squash").run({ a, b }, function(path, why) made = { path = path and s.relative(path), why = why } end)
    vim.wait(5000, function() return made ~= nil end, 20)
    io.write(vim.json.encode({ made = made, text = made.path and io.open(s.absolute(made.path)):read("*a"), a = vim.uv.fs_stat(a) ~= nil, b = vim.uv.fs_stat(b) ~= nil }))
  `);
  assert.equal(result.made.why, undefined);
  assert.equal(result.made.path, "Groceries and errands.md");
  assert.match(result.text, /^# Groceries and errands\n\n- \[ \] milk\n- \[ \] post office\n\n<!-- note-id: (n-[0-9a-f]{12}) -->\n$/, "with an ID of its own");
  assert.equal(result.a, false, "the originals leave the notes");
  assert.equal(result.b, false);
  assert.ok(fs.existsSync(path.join(dir, ".trash", "Groceries.md")) && fs.existsSync(path.join(dir, ".trash", "Errands.md")), "to Recently Deleted");
  const given = fs.readFileSync(asked, "utf8");
  assert.match(given, /^-p\n--model\nsonnet\n--tools\n\n--no-session-persistence\n/);
  assert.match(given, /<note 1 changed="2026-01-01T00:00:00Z">\n# Groceries[\s\S]*<note 2 changed="\d{4}-[^"]+">\n# Errands/, "each note with when it was last changed");
  assert.match(given, /keep the version from the note changed most recently/);
  assert.doesNotMatch(given, /note-id/, "no note's ID is sent");
  assert.notEqual(result.text.match(/note-id: (n-[0-9a-f]{12})/)[1], "n-aaaaaaaaaaaa", "the merged note's ID is its own");
});

test("tabs linked to squashed notes link to the new note, and the IDs the work-a-note mode keeps for any of them become the new note's", (t) => {
  const bin = fs.mkdtempSync(path.join(tmp, "claude-bin-"));
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat >/dev/null\nprintf '# Both\\n\\n- one\\n- two\\n'\n`, { mode: 0o755 });
  const socket = path.join(fs.mkdtempSync("/tmp/sq-"), "tmux.sock");
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "sleep 60"]);
  const pid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
  const window = execFileSync("tmux", ["-S", socket, "display", "-p", "#{window_id}"], { encoding: "utf8" }).trim();
  const other = execFileSync("tmux", ["-S", socket, "new-window", "-d", "-P", "-F", "#{window_id}", "sleep 60"], { encoding: "utf8" }).trim();
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", window, "@tab_note", "One.md"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", other, "@tab_note_last", "Two.md"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", other, "@tab_name", "Two"]);
  const saved = { PATH: process.env.PATH, TMUX: process.env.TMUX };
  process.env.PATH = `${bin}:${saved.PATH}`;
  process.env.TMUX = `${socket},${pid},0`;
  const state = fs.mkdtempSync(path.join(tmp, "state-"));
  process.env.TMUX_SIDEBAR_STATE_DIR = state;
  t.after(() => {
    process.env.PATH = saved.PATH;
    delete process.env.TMUX;
    delete process.env.TMUX_SIDEBAR_STATE_DIR;
    execFileSync("tmux", ["-S", socket, "kill-server"]);
  });
  const { result } = lua(`
    local s = require("notes.store")
    local a = s.create("", "One")
    local b = s.create("", "Two")
    -- The mode works Two now, with a tab still open for an item of One,
    -- which it worked before, and an item of One skipped.
    local id = require("notes.id").of_file(a)
    local f = io.open("${state}/autopilot.json", "w")
    f:write(vim.json.encode({ on = true, note = "Two.md", noteId = require("notes.id").of_file(b), skipped = { { note = id, text = "skipped" } } }))
    f:close()
    vim.system({ "tmux", "set", "-w", "-t", "${window}", "@autopilot_note", id }):wait()
    local done
    require("notes.squash").run({ a, b }, function(path) done = path or false end)
    vim.wait(5000, function() return done ~= nil end, 20)
    io.write(vim.json.encode({ old = id, new = done and require("notes.id").of_file(done) }))
  `);
  const mode = JSON.parse(fs.readFileSync(path.join(state, "autopilot.json"), "utf8"));
  assert.ok(result.new && result.new !== result.old);
  assert.equal(mode.note, "Both.md");
  assert.equal(mode.noteId, result.new, "the mode's note by the merged note's ID");
  assert.deepEqual(mode.skipped, [{ note: result.new, text: "skipped" }], "its skipped item with it");
  assert.equal(execFileSync("tmux", ["-S", socket, "show", "-wqv", "-t", window, "@autopilot_note"], { encoding: "utf8" }).trim(), result.new, "and its tab's, opened for the other squashed note");
  const show = (w, option) => execFileSync("tmux", ["-S", socket, "show", "-wqv", "-t", w, option], { encoding: "utf8" }).trim();
  assert.equal(show(window, "@tab_note"), "Both.md");
  assert.equal(show(other, "@tab_note_last"), "Both.md");
  assert.equal(show(other, "@tab_name"), "Both", "a tab named after a squashed note takes the new one's title");
});

test("a note renamed from the sidebar takes its tabs' links along, and a tab named for it its new title", (t) => {
  const socket = path.join(fs.mkdtempSync("/tmp/rn-"), "tmux.sock");
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "sleep 60"]);
  const pid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
  const named = execFileSync("tmux", ["-S", socket, "display", "-p", "#{window_id}"], { encoding: "utf8" }).trim();
  const own = execFileSync("tmux", ["-S", socket, "new-window", "-d", "-P", "-F", "#{window_id}", "sleep 60"], { encoding: "utf8" }).trim();
  const beside = execFileSync("tmux", ["-S", socket, "new-window", "-d", "-P", "-F", "#{window_id}", "sleep 60"], { encoding: "utf8" }).trim();
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", named, "@tab_note", "Plans.md"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", named, "@tab_name", "Plans"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", own, "@tab_note", "Plans.md"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", own, "@tab_name", "Release work"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", beside, "@tab_note_last", "Work/Ideas.md"]);
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", beside, "@tab_note_arrived", "Work/Ideas.md"]);
  process.env.TMUX = `${socket},${pid},0`;
  t.after(() => {
    delete process.env.TMUX;
    execFileSync("tmux", ["-S", socket, "kill-server"]);
  });
  const { result } = lua(`
    local s = require("notes.store")
    local plans = s.create("", "Plans")
    local f = io.open(plans, "a") f:write("- [ ] ship it\\n") f:close()
    vim.fn.mkdir(s.absolute("Work"), "p")
    s.create("Work", "Ideas")
    local to = require("notes").retitle(plans, "Weekend plans")
    local text = io.open(to):read("*a")
    local folder = s.rename_folder("Work", "Home")
    -- A title ending in a number, and one whose file keeps its name (a : and
    -- a - make the same file name), still rename the tab named for it.
    local numbered = require("notes").retitle(to, "Plans 2")
    local same_file = require("notes").retitle(numbered, "Plans: 2")
    io.write(vim.json.encode({ to = s.relative(to), folder = folder, text = text, same_file = s.relative(same_file) }))
  `);
  const show = (w, option) => execFileSync("tmux", ["-S", socket, "show", "-wqv", "-t", w, option], { encoding: "utf8" }).trim();
  assert.equal(result.to, "Weekend plans.md", "the file is named for the new title");
  assert.match(result.text, /^# Weekend plans\n\n- \[ \] ship it\n\n<!-- note-id: n-[0-9a-f]{12} -->\n$/, "only the title changed");
  assert.equal(result.same_file, "Plans- 2.md");
  assert.equal(show(named, "@tab_note"), "Plans- 2.md");
  assert.equal(show(named, "@tab_name"), "Plans: 2", "a tab named for the note takes each new title");
  assert.equal(show(own, "@tab_note"), "Plans- 2.md");
  assert.equal(show(own, "@tab_name"), "Release work", "a tab with a name of its own keeps it");
  assert.equal(show(beside, "@tab_note_last"), "Home/Ideas.md", "a renamed folder takes its notes' links along");
  assert.equal(show(beside, "@tab_note_arrived"), "Home/Ideas.md", "and the note Notes came beside the tab with");
});

test("a squash asks in a box first, then shows a box while Claude works, with the notes and the time so far, and Esc in it cancels, leaving the notes as they were", (t) => {
  const bin = fs.mkdtempSync(path.join(tmp, "claude-bin-"));
  // A stand-in claude that takes its time.
  fs.writeFileSync(path.join(bin, "claude"), "#!/bin/sh\ncat >/dev/null\nsleep 5\nprintf '# Both\\n'\n", { mode: 0o755 });
  const saved = { PATH: process.env.PATH, TMUX: process.env.TMUX };
  process.env.PATH = `${bin}:${saved.PATH}`;
  delete process.env.TMUX;
  t.after(() => { process.env.PATH = saved.PATH; });
  const { result, dir } = lua(`
    local s = require("notes.store")
    local sidebar = require("notes.sidebar")
    local a = s.create("", "First")
    local b = s.create("", "Second")
    sidebar.state.marked = { a, b }
    local said = {}
    vim.notify = function(msg) table.insert(said, msg) end
    local box = function()
      for _, w in ipairs(vim.api.nvim_list_wins()) do
        if vim.api.nvim_win_get_config(w).relative ~= "" then return table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n") end
      end
    end
    local press = function(key) vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(key, true, false, true), "x", false) end
    -- Asked first, in a box: n keeps the notes.
    sidebar.squash()
    local asked = box()
    press("n")
    local kept = box() == nil
    -- Enter squashes.
    sidebar.squash()
    press("<CR>")
    local shown = box()
    vim.wait(1200)
    local later = box()
    vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes("<Esc>", true, false, true), "x", false)
    vim.wait(1500, function() return box() == nil end, 20)
    vim.wait(300)
    io.write(vim.json.encode({ asked = asked, kept = kept, shown = shown, later = later, gone = box() == nil, a = vim.uv.fs_stat(a) ~= nil, b = vim.uv.fs_stat(b) ~= nil, said = said }))
  `);
  assert.match(result.asked, /Squash these 2 notes into one\?[\s\S]*· First[\s\S]*· Second[\s\S]*Claude \(sonnet\)[\s\S]*Recently Deleted[\s\S]*\[Y\/n\]/, "asked in a box, Enter as yes");
  assert.equal(result.kept, true, "n closes it, squashing nothing");
  assert.match(result.shown, /Squashing 2 notes with Claude \(sonnet\)/);
  assert.match(result.shown, /· First[\s\S]*· Second/);
  assert.match(result.shown, /Esc cancels/);
  assert.match(result.later, /[1-9]s so far/, "the time counts up");
  assert.equal(result.gone, true, "Esc closes it");
  assert.equal(result.a && result.b, true, "the notes are untouched");
  assert.ok(result.said.some((m) => /canceled/.test(m)));
  assert.ok(!fs.existsSync(path.join(dir, "Both.md")));
});

test("a note opened beside a tab, other than the one Notes came with, is the tab's at once, and going back to the first counts too", (t) => {
  const socket = path.join(fs.mkdtempSync("/tmp/ob-"), "tmux.sock");
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "sleep 60"]);
  const pid = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pid}"], { encoding: "utf8" }).trim();
  const window = execFileSync("tmux", ["-S", socket, "display", "-p", "#{window_id}"], { encoding: "utf8" }).trim();
  const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pane_id}"], { encoding: "utf8" }).trim();
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", window, "@tab_note_arrived", "A.md"]);
  process.env.TMUX = `${socket},${pid},0`;
  process.env.TMUX_PANE = pane;
  t.after(() => {
    delete process.env.TMUX;
    delete process.env.TMUX_PANE;
    execFileSync("tmux", ["-S", socket, "kill-server"]);
  });
  const show = (option) => execFileSync("tmux", ["-S", socket, "show", "-wqv", "-t", window, option], { encoding: "utf8" }).trim();
  lua(`
    require("notes.store").opened_beside("A.md")
    vim.wait(300)
    io.write(vim.json.encode({}))
  `);
  assert.deepEqual([show("@tab_note_arrived"), show("@tab_note_last")], ["A.md", ""], "the note it came with isn't the tab's");
  lua(`
    require("notes.store").opened_beside("B.md")
    vim.wait(2000, function() return vim.fn.system({ "tmux", "show", "-wqv", "@tab_note_last" }) ~= "" end, 20)
    require("notes.store").opened_beside("A.md")
    vim.wait(2000, function() return vim.fn.system({ "tmux", "show", "-wqv", "@tab_note_last" }) == "A.md\\n" end, 20)
    io.write(vim.json.encode({}))
  `);
  assert.deepEqual([show("@tab_note_arrived"), show("@tab_note_last")], ["", "A.md"], "another is, and then going back to the first");
  execFileSync("tmux", ["-S", socket, "set", "-w", "-t", window, "@notes", "1"]);
  lua(`
    require("notes.store").opened_beside("C.md")
    vim.wait(300)
    io.write(vim.json.encode({}))
  `);
  assert.equal(show("@tab_note_last"), "A.md", "never in Notes' own tab");
});

test("an image pasted into a note is saved beside it and written under the item, shown by its name, opened only from inside the notes", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local path = s.create("", "Bug report")
    local f = io.open(path, "w"); f:write("# Bug report\\n\\n- [ ] the button is cut off\\n"); f:close()
    vim.cmd("Notes")
    local n = require("notes")
    n.open(path)
    local buf = vim.api.nvim_get_current_buf()
    local images = require("notes.images")
    -- The clipboard stood in for: a PNG's bytes written where asked.
    images.grab = function(file) local out = io.open(file, "wb"); out:write("\\137PNG"); out:close(); return true end
    vim.api.nvim_win_set_cursor(0, { 3, 0 })
    local rel = images.paste(buf)
    local line = vim.api.nvim_buf_get_lines(buf, 3, 4, false)[1]
    local spans = require("notes.render").inline(line)
    local image = vim.tbl_filter(function(sp) return sp.image end, spans)[1]
    local opened
    images.open = function(file) opened = file end
    local followed = require("notes.links").follow(buf, 4, image.s)
    local outside = images.file(buf, "../../etc/passwd")
    -- Ctrl-V while typing: the image pasted, as in Claude Code.
    vim.api.nvim_win_set_cursor(0, { 3, 0 })
    vim.api.nvim_feedkeys(vim.keycode("A<C-v><Esc>"), "xt", false)
    local typed = vim.api.nvim_buf_get_lines(buf, 3, 4, false)[1]
    images.grab = function() return false end
    -- With none on the clipboard, Ctrl-V is Neovim's own: the next key as it is.
    vim.api.nvim_win_set_cursor(0, { 3, 0 })
    vim.api.nvim_feedkeys(vim.keycode("A<C-v><Tab><Esc>"), "xt", false)
    local literal = vim.api.nvim_buf_get_lines(buf, 2, 3, false)[1]
    local _, why = images.paste(buf)
    io.write(vim.json.encode({ rel = rel, line = line, shown = line:sub(image.s + 1, image.e), followed = followed, opened = opened, outside = outside == nil, why = why, typed = typed, literal = literal }))
  `);
  assert.match(result.rel, /^\.attachments\/Bug-report\/image-\d{4}-\d{2}-\d{2}-\d{6}\.png$/, "beside the note, in a folder the list leaves out, with no space in its path");
  assert.equal(result.line, `  ![${path.basename(result.rel, ".png")}](${result.rel})`, "on a line of its own, indented under the item");
  assert.ok(fs.existsSync(path.join(dir, result.rel)), "the image saved");
  assert.equal(result.shown, path.basename(result.rel, ".png"), "shown by its name, the markup hidden");
  assert.equal(result.followed, true);
  assert.equal(result.opened, fs.realpathSync(path.join(dir, result.rel)), "Enter opens the image");
  assert.equal(result.outside, true, "never a file outside the notes");
  assert.equal(result.why, "no image on the clipboard");
  assert.match(result.typed, /^  !\[image-.*\]\(\.attachments\/Bug-report\/image-.*\.png\)$/, "Ctrl-V while typing pastes the clipboard's image under the line");
  assert.equal(result.literal, "- [ ] the button is cut off\t", "and with none there, types the next key as it is");
});

test("an image under a checklist item moves with it when an item is ticked, and with its note to another folder", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local path = s.create("", "Bugs")
    local f = io.open(path, "w"); f:write("# Bugs\\n\\n- [ ] first\\n- [ ] second\\n  ![shot](.attachments/Bugs/shot.png)\\n"); f:close()
    vim.fn.mkdir(vim.fs.dirname(path) .. "/.attachments/Bugs", "p")
    local img = io.open(vim.fs.dirname(path) .. "/.attachments/Bugs/shot.png", "wb"); img:write("\\137PNG"); img:close()
    vim.cmd("Notes")
    local n = require("notes")
    n.open(path)
    local buf = vim.api.nvim_get_current_buf()
    require("notes.format").toggle_check(buf, 3)
    local sorted = vim.api.nvim_buf_get_lines(buf, 2, -1, false)
    -- Deleting the ticked ones takes what is under each with it.
    local other = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_lines(other, 0, -1, false, { "- [ ] keep", "", "- [x] done", "  ![old](.attachments/Bugs/old.png)" })
    require("notes.format").delete_checked(other)
    local deleted = vim.api.nvim_buf_get_lines(other, 0, -1, false)
    n.save(buf)
    local moved = s.move(path, "Work")
    local data = require("notes.images").resolve(moved, "data:image/png;base64,iVBORw0KGgo=")
    s.delete(moved)
    io.write(vim.json.encode({ sorted = sorted, deleted = deleted, moved = s.relative(moved), data = data == nil }))
  `);
  assert.deepEqual(result.sorted.slice(0, 4), ["- [ ] second", "  ![shot](.attachments/Bugs/shot.png)", "", "- [x] first"], "the image stays under its item");
  assert.deepEqual(result.deleted, ["- [ ] keep"], "a ticked item deleted with its image line, which isn't left under another");
  assert.equal(result.moved, "Work/Bugs.md");
  assert.ok(fs.existsSync(path.join(dir, "Work/.attachments/Bugs/shot.png")), "the image went along with the note");
  assert.ok(fs.existsSync(path.join(dir, ".attachments/Bugs/shot.png")), "copied, so a note still naming it where it was keeps it");
  assert.ok(fs.existsSync(path.join(dir, "Work/.attachments/Bugs/shot.png")), "a deleted note's image stays, for a note still naming it (a squash's)");
  assert.ok(fs.existsSync(path.join(dir, ".trash/Work/.attachments/Bugs/shot.png")), "and goes with it to Recently Deleted, where it still shows");
  assert.equal(result.data, true, "never a data: address");
});

test("squashing notes from two folders copies the images the merged note names into its folder, and only images inside the notes are drawn", (t) => {
  const bin = fs.mkdtempSync(path.join(tmp, "claude-bin-"));
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat >/dev/null\nprintf '# Both\\n\\n- [ ] one\\n- [ ] two\\n  ![shot](.attachments/Two/shot.png)\\n'\n`, { mode: 0o755 });
  const saved = process.env.PATH;
  process.env.PATH = `${bin}:${saved}`;
  t.after(() => { process.env.PATH = saved; });
  const { result, dir } = lua(`
    local s = require("notes.store")
    local a = s.create("A", "One")
    local f = io.open(a, "w"); f:write("# One\\n\\n- [ ] one\\n"); f:close()
    local b = s.create("B", "Two")
    local g = io.open(b, "w"); g:write("# Two\\n\\n- [ ] two\\n  ![shot](.attachments/Two/shot.png)\\n"); g:close()
    vim.fn.mkdir(vim.fs.dirname(b) .. "/.attachments/Two", "p")
    local img = io.open(vim.fs.dirname(b) .. "/.attachments/Two/shot.png", "wb"); img:write("\\137PNG"); img:close()
    local outside = io.open(s.root() .. "/../outside.png", "wb"); outside:write("x"); outside:close()
    local made
    require("notes.squash").run({ a, b }, function(path) made = path or false end)
    vim.wait(5000, function() return made ~= nil end, 20)
    local resolve = require("notes.images").resolve
    io.write(vim.json.encode({
      made = made and s.relative(made),
      drawn = resolve(made, ".attachments/Two/shot.png") ~= nil,
      up = resolve(made, "../../outside.png") == nil,
      data = resolve(made, "data:image/png;base64,iVBORw0KGgo=") == nil,
    }))
  `);
  assert.equal(result.made, "A/Both.md");
  assert.ok(fs.existsSync(path.join(dir, "A/.attachments/Two/shot.png")), "the image copied into the merged note's folder");
  assert.equal(result.drawn, true, "a picture beside the note is drawn");
  assert.equal(result.up, true, "never one outside the notes");
  assert.equal(result.data, true, "nor a data: address");
});

test("an image is drawn under its line as kitty placeholder cells, sized to the window and coloured with its id, and only a PNG", () => {
  const { result } = lua(`
    -- A terminal that speaks kitty's graphics protocol, what is sent to it kept.
    vim.env.TERM = "xterm-ghostty"
    local seqs = {}
    vim.api.nvim_ui_send = function(seq) table.insert(seqs, seq) end
    local s = require("notes.store")
    local path = s.create("", "Shots")
    vim.fn.mkdir(vim.fs.dirname(path) .. "/.attachments/Shots", "p")
    -- A PNG header saying 1300 by 280 pixels, which is all the drawing reads.
    local png = "\\137PNG\\r\\n\\26\\n" .. "\\0\\0\\0\\13IHDR" .. "\\0\\0\\5\\20" .. "\\0\\0\\1\\24"
    local img = io.open(vim.fs.dirname(path) .. "/.attachments/Shots/a.png", "wb"); img:write(png); img:close()
    local txt = io.open(vim.fs.dirname(path) .. "/.attachments/Shots/b.png", "wb"); txt:write("not a png"); txt:close()
    -- A JPEG, made from a real 1-by-1 PNG with macOS's sips.
    local dot = io.open(vim.fs.dirname(path) .. "/dot.png", "wb"); dot:write(vim.base64.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8DwHwAFBQIAX8jx0gAAAABJRU5ErkJggg==")); dot:close()
    vim.system({ "sips", "-s", "format", "jpeg", vim.fs.dirname(path) .. "/dot.png", "--out", vim.fs.dirname(path) .. "/.attachments/Shots/c.jpg" }):wait()
    local f = io.open(path, "w"); f:write("# Shots\\n\\n![a](.attachments/Shots/a.png)\\n![b](.attachments/Shots/b.png)\\n![c](.attachments/Shots/c.jpg) ![c2](.attachments/Shots/c.jpg)\\n\\n\`\`\`\\n![a](.attachments/Shots/a.png)\\n\`\`\`\\n"); f:close()
    vim.cmd("Notes")
    local n = require("notes")
    n.open(path)
    local buf = vim.api.nvim_get_current_buf()
    local images = require("notes.images")
    images.draw(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    -- A colorscheme loaded clears highlights: the pictures' are made again.
    vim.cmd("hi clear")
    vim.cmd("doautocmd ColorScheme")
    vim.wait(200)
    local marks = vim.api.nvim_buf_get_extmarks(buf, images.ns, 0, -1, { details = true })
    local m = marks[1]
    local cells = m[4].virt_lines[1][2][1]
    local group = m[4].virt_lines[1][2][2]
    -- Drawn again at the same size: nothing sent again.
    local count = #seqs
    images.draw(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    local resent = #seqs - count
    -- An image taken out of the note is taken back from the terminal.
    local before = #seqs
    vim.api.nvim_buf_set_lines(buf, 2, 3, false, {})
    images.draw(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    local released = table.concat(vim.list_slice(seqs, before + 1), ""):find("a=d,d=I", 1, true) ~= nil
    -- In a terminal that doesn't, an image is its name alone.
    vim.env.TERM = "xterm-256color"
    vim.cmd("doautocmd FocusGained")
    vim.wait(200)
    local plain = #vim.api.nvim_buf_get_extmarks(buf, images.ns, 0, -1, {})
    io.write(vim.json.encode({
      plain = plain, released = released, resent = resent,
      count = #marks, row = m[2], jpeg = marks[2] and marks[2][2], twice = marks[2] and #marks[2][4].virt_lines, rows = #m[4].virt_lines,
      cols = vim.fn.strchars(cells) / 3, first = vim.fn.char2nr(vim.fn.strcharpart(cells, 0, 1)),
      fg = vim.api.nvim_get_hl(0, { name = group }).fg ~= nil,
    }))
  `);
  assert.equal(result.count, 2, "the PNG and the JPEG drawn, the file that isn't a picture not");
  assert.equal(result.jpeg, 4, "a JPEG made a PNG to draw, under its own line");
  assert.equal(result.twice % 2, 0, "and two images a line names both drawn under it");
  assert.ok(result.twice >= 2);
  assert.equal(result.row, 2, "under its own line");
  assert.equal(result.plain, 0, "and none in a terminal that can't draw them");
  assert.equal(result.released, true, "an image taken out of the note is taken back from the terminal");
  assert.equal(result.resent, 0, "a picture already sent isn't sent again when the note is drawn again");
  assert.equal(result.first, 0x10eeee, "in placeholder cells");
  assert.ok(result.cols > 0 && result.cols < Math.ceil(1300 / 8), "as wide as the window has room for, narrower than the picture at a common cell size");
  assert.equal(result.rows, Math.ceil((result.cols * 8 * 280) / 1300 / 16), "and as tall as its shape says");
  assert.equal(result.fg, true, "coloured with its id");
});

test("only the pictures on screen or near it are given to the terminal, the rest once scrolled to", () => {
  const { result } = lua(`
    vim.env.TERM = "xterm-ghostty"
    local seqs = {}
    vim.api.nvim_ui_send = function(seq) table.insert(seqs, seq) end
    local s = require("notes.store")
    local path = s.create("", "Long")
    vim.fn.mkdir(vim.fs.dirname(path) .. "/.attachments/Long", "p")
    local png = "\\137PNG\\r\\n\\26\\n" .. "\\0\\0\\0\\13IHDR" .. "\\0\\0\\0\\100" .. "\\0\\0\\0\\50"
    local img = io.open(vim.fs.dirname(path) .. "/.attachments/Long/far.png", "wb"); img:write(png); img:close()
    local text = { "# Long", "" }
    for i = 1, 300 do text[#text + 1] = "line " .. i end
    text[#text + 1] = "![far](.attachments/Long/far.png)"
    local f = io.open(path, "w"); f:write(table.concat(text, "\\n") .. "\\n"); f:close()
    vim.cmd("Notes")
    require("notes").open(path)
    local buf = vim.api.nvim_get_current_buf()
    local images = require("notes.images")
    local function sent() return #vim.tbl_filter(function(q) return q:find("a=T", 1, true) ~= nil end, seqs) end
    images.draw(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    local far = sent()
    vim.cmd("normal! G")
    images.draw(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    io.write(vim.json.encode({ far = far, near = sent(), marks = #vim.api.nvim_buf_get_extmarks(buf, images.ns, 0, -1, {}) }))
  `);
  assert.equal(result.far, 0, "a picture far below the screen isn't sent");
  assert.equal(result.near, 1, "it is once scrolled to");
  assert.equal(result.marks, 1, "its lines kept either way");
});

test("the toolbar has a button for each of its Format menu items, a click styles the word or the selection, and easy mode adds the Cmd keys", () => {
  const { result } = lua(`
    -- A config's Visual mode map of a character, as nvim/init.lua's J.
    vim.keymap.set("v", "J", ":m '>+1<CR>gv=gv")
    vim.g.mapleader = " "
    vim.keymap.set("v", "<leader>f", "<Nop>")
    -- And one a FileType autocommand makes for the note's buffer.
    vim.api.nvim_create_autocmd("FileType", { pattern = "notes", callback = function(a) vim.keymap.set("v", "K", "<Nop>", { buffer = a.buf }) end })
    local easy = require("notes.easy")
    easy.settings = function() return { notes_toolbar = true, easy_keys = true, key_help_line = true } end
    vim.cmd("Notes")
    local n = require("notes")
    local buf = vim.api.nvim_win_get_buf(n.note_win)
    vim.cmd("stopinsert")
    local labels = vim.tbl_map(function(item) return item[2] end, n.format_items(buf, 1, 1, false))
    local missing = vim.tbl_filter(function(b) return not b.menu and not vim.tbl_contains(labels, b.item) end, easy.BUTTONS)
    local _, regions = easy.toolbar(nil, 200):gsub("@v:lua.notes_toolbar_click@", "")
    local _, shown = vim.wo[n.note_win].winbar:gsub("@v:lua.notes_toolbar_click@", "")
    -- In a window too narrow for them all, Notes beside a pane: the first
    -- buttons and ⋯, and nothing past the window's edge.
    local narrow, narrow_spans = easy.toolbar(nil, 30)
    local last_col = 0
    for _, span in pairs(narrow_spans) do last_col = math.max(last_col, span[2]) end
    local narrow_buttons = {}
    for i in pairs(narrow_spans) do table.insert(narrow_buttons, i) end
    table.sort(narrow_buttons)
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, { "# Title", "", "hello big world", "one two" })
    vim.api.nvim_win_set_cursor(n.note_win, { 3, 1 })
    easy.click(1)
    vim.wait(50, function() return false end)
    vim.cmd("stopinsert")
    vim.api.nvim_win_set_cursor(n.note_win, { 4, 0 })
    vim.api.nvim_feedkeys(vim.keycode("vee<Cmd>lua require('notes.easy').click(2)<CR>"), "x", false)
    vim.wait(50, function() return false end)
    io.write(vim.json.encode({
      missing = #missing, regions = regions, buttons = #easy.BUTTONS, narrow_buttons = narrow_buttons, last_col = last_col, shown = shown,
      word = vim.api.nvim_buf_get_lines(buf, 2, 3, false)[1], selection = vim.api.nvim_buf_get_lines(buf, 3, 4, false)[1],
      keymodel = vim.o.keymodel, select_j = vim.fn.maparg("J", "s"), select_leader = vim.fn.maparg(" f", "s"), select_buffer_k = vim.fn.maparg("K", "s"), visual_buffer_k = vim.fn.maparg("K", "x"), visual_j = vim.fn.maparg("J", "x"), copy = vim.fn.maparg("<C-c>", "v"), save = vim.fn.maparg("<C-M-S-s>", "n", false, true).desc, help = vim.o.statusline,
    }))
  `);
  assert.equal(result.missing, 0, "every button names an item of the Format menu");
  assert.equal(result.regions, result.buttons, "one click region per button, given the room");
  assert.ok(result.shown >= 2, "the note window shows the buttons that fit it");
  assert.equal(result.narrow_buttons[0], 1, "a narrow window keeps the first buttons");
  assert.equal(result.narrow_buttons.at(-1), result.buttons, "and ⋯, for the rest");
  assert.ok(result.narrow_buttons.length < result.buttons);
  assert.ok(result.last_col <= 30, "nothing past the window's edge");
  assert.equal(result.word, "**hello** big world", "B bolds the word under the cursor");
  assert.equal(result.selection, "*one two*", "I italicizes the selection");
  assert.equal(result.keymodel, "startsel,stopsel", "Shift-arrows select in easy mode");
  assert.equal(result.save, "Save (Cmd-S)");
  assert.equal(result.select_j, "", "J typed over a selection replaces it");
  assert.equal(result.select_leader, "", "and so does Space f, with Space as the leader");
  assert.equal(result.select_buffer_k, "", "and a key the note's own buffer maps");
  assert.equal(result.visual_buffer_k, "<Nop>");
  assert.notEqual(result.visual_j, "", "the config's J still moves lines in Visual mode");
  assert.equal(result.copy, '"+y', "Ctrl-C copies a selection");
  assert.match(result.help, /notes_key_help/);
});

test("in easy mode, the Format menu has a close button in its border's top-right corner, which a click on closes", () => {
  const { result } = lua(`
    -- As nvim/init.lua sets it, which every float would take.
    vim.o.winborder = "rounded"
    local easy = require("notes.easy")
    easy.settings = function() return { notes_toolbar = true, easy_keys = true, key_help_line = true } end
    vim.cmd("Notes")
    local n = require("notes")
    local buf = vim.api.nvim_win_get_buf(n.note_win)
    vim.cmd("stopinsert")
    n.menu(buf, false)
    local menu = vim.api.nvim_get_current_win()
    local button
    for _, w in ipairs(vim.api.nvim_list_wins()) do
      if w ~= menu and vim.api.nvim_win_get_config(w).relative == "win" then button = w end
    end
    local mpos, bpos = vim.api.nvim_win_get_position(menu), vim.api.nvim_win_get_position(button)
    local mwidth = vim.api.nvim_win_get_width(menu)
    local text = vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(button), 0, -1, false)[1]
    local own_border = vim.api.nvim_win_get_config(button).border
    local press = vim.fn.maparg("<LeftMouse>", "n", false, true).callback
    -- A click elsewhere goes on as a click.
    vim.fn.getmousepos = function() return { screenrow = bpos[1] + 2, screencol = bpos[2] + 2 } end
    local elsewhere = press()
    vim.fn.getmousepos = function() return { screenrow = bpos[1] + 1, screencol = bpos[2] + 2 } end
    local on = press()
    vim.wait(50, function() return false end)
    -- Tab puts focus on it, and Enter then presses it rather than taking the
    -- line the cursor is on.
    n.menu(buf, false)
    local again = vim.api.nvim_get_current_win()
    local before = table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\\n")
    vim.fn.maparg("<Tab>", "n", false, true).callback()
    vim.fn.maparg("<CR>", "n", false, true).callback()
    local tabbed = { open = vim.api.nvim_win_is_valid(again), same = table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\\n") == before }
    n.menu(buf, false)
    local spaced = vim.api.nvim_get_current_win()
    vim.fn.maparg("<Tab>", "n", false, true).callback()
    vim.fn.maparg("<Space>", "n", false, true).callback()
    tabbed.space_open = vim.api.nvim_win_is_valid(spaced)
    io.write(vim.json.encode({
      tabbed = tabbed, own_border = own_border or "none",
      text = text, row = bpos[1] - mpos[1], right = mpos[2] + mwidth + 2 - (bpos[2] + 3),
      elsewhere = elsewhere, on = on, menu_open = vim.api.nvim_win_is_valid(menu), button_open = vim.api.nvim_win_is_valid(button),
      back = vim.api.nvim_get_current_win() == n.note_win,
    }))
  `);
  assert.equal(result.text, " × ");
  assert.equal(result.row, 0, "on the border's top edge");
  assert.ok(result.own_border === "none" || (Array.isArray(result.own_border) && result.own_border.every((c) => c === "")), "with no border of its own, whatever winborder says");
  assert.equal(result.right, 1, "a cell short of its corner");
  assert.equal(result.elsewhere, "<LeftMouse>");
  assert.equal(result.on, "");
  assert.equal(result.menu_open, false, "a click on it closes the menu");
  assert.equal(result.button_open, false, "and the button with it");
  assert.equal(result.back, true);
  assert.deepEqual(result.tabbed, { open: false, same: true, space_open: false }, "Tab then Enter, or Space, closes it, doing nothing to the note");
});

test("with the toolbar and easy mode off, a note has no toolbar and Vim's keys alone", () => {
  const { result } = lua(`
    require("notes.easy").settings = function() return { notes_toolbar = false, easy_keys = false, key_help_line = true } end
    vim.cmd("Notes")
    local n = require("notes")
    io.write(vim.json.encode({ bar = vim.wo[n.note_win].winbar, keymodel = vim.o.keymodel, save = vim.fn.maparg("<C-M-S-s>", "n") }))
  `);
  assert.deepEqual(result, { bar: "", keymodel: "", save: "" });
});

test("Notes reads the toolbar and easy mode settings from tmux, as tmux/settings.sh sets them, and marks its pane for the Cmd keys while it runs", () => {
  const socket = path.join(tmp, "easy.sock");
  execFileSync("tmux", ["-S", socket, "-f", "/dev/null", "new-session", "-d", "cat"]);
  try {
    execFileSync("tmux", ["-S", socket, "set", "-g", "@setting_easy_keys", "on"]);
    execFileSync("tmux", ["-S", socket, "set", "-g", "@setting_notes_toolbar", "off"]);
    const pane = execFileSync("tmux", ["-S", socket, "display", "-p", "#{pane_id}"], { encoding: "utf8" }).trim();
    const { result } = lua(`
      vim.env.TMUX = ${JSON.stringify(socket)} .. ",1,0"
      vim.env.TMUX_PANE = ${JSON.stringify(pane)}
      local easy = require("notes.easy")
      local settings = easy.settings()
      easy.start()
      local marked = vim.trim(vim.fn.system({ "tmux", "show", "-pqv", "-t", vim.env.TMUX_PANE, "@notes_cmd_keys" }))
      vim.api.nvim_exec_autocmds("VimLeavePre", {})
      local left = vim.trim(vim.fn.system({ "tmux", "show", "-pqv", "-t", vim.env.TMUX_PANE, "@notes_cmd_keys" }))
      io.write(vim.json.encode({ settings = settings, marked = marked, left = left }))
    `);
    assert.deepEqual(result.settings, { notes_toolbar: false, easy_keys: true, key_help_line: true, hover_highlight: true });
    assert.equal(result.marked, "1", "easy mode marks its pane, for tmux to pass it the Cmd keys");
    assert.equal(result.left, "", "and the mark goes with Notes");
  } finally {
    execFileSync("tmux", ["-S", socket, "kill-server"]);
  }
});

test("a note renamed takes the mode working it continuously along, which keeps its other state", (t) => {
  const state = fs.mkdtempSync(path.join(tmp, "autopilot-state-"));
  fs.writeFileSync(path.join(state, "autopilot.json"), JSON.stringify({ on: true, note: "Plans.md", repo: "/r", slots: 3, cap: 5, skipped: [], said: "Can't find the note Plans: set the mode up again from M-p" }, null, 2));
  process.env.TMUX_SIDEBAR_STATE_DIR = state;
  t.after(() => delete process.env.TMUX_SIDEBAR_STATE_DIR);
  lua(`
    local s = require("notes.store")
    local plans = s.create("", "Plans")
    require("notes").retitle(plans, "Weekend plans")
    io.write("{}")
  `);
  const mode = JSON.parse(fs.readFileSync(path.join(state, "autopilot.json"), "utf8"));
  assert.deepEqual([mode.note, mode.on, mode.repo, mode.slots, mode.cap, mode.said], ["Weekend plans.md", true, "/r", 3, 5, ""]);
});

test("each note has a stable ID as its last line: made with it, given one when first opened without it, kept last on save, put back when deleted, hidden, and a copy gets one of its own", () => {
  const { result, dir } = lua(`
    local s = require("notes.store")
    local ids = require("notes.id")
    local n = require("notes")
    local made = s.create("", "Fresh")
    local made_text = io.open(made):read("*a")
    -- An old note with no ID, last changed long ago.
    local old = s.root() .. "/Old.md"
    local f = io.open(old, "w"); f:write("# Old\\n\\n- [ ] one\\n"); f:close()
    vim.uv.fs_utime(old, 1767225600, 1767225600)
    vim.cmd("Notes")
    n.open(old)
    local buf = vim.api.nvim_get_current_buf()
    local given = io.open(old):read("*a")
    local id = ids.in_text(given)
    local kept_time = vim.uv.fs_stat(old).mtime.sec
    -- Hidden: the whole line concealed.
    require("notes.render").draw(buf)
    local last = vim.api.nvim_buf_line_count(buf) - 1
    local concealed = false
    for _, m in ipairs(vim.api.nvim_buf_get_extmarks(buf, require("notes.render").ns, { last, 0 }, { last, -1 }, { details = true })) do
      if m[4].conceal == "" and m[4].end_col == #vim.api.nvim_buf_get_lines(buf, last, last + 1, false)[1] then concealed = true end
    end
    -- Typed below it: the save moves it last.
    vim.api.nvim_buf_set_lines(buf, -1, -1, false, { "- [ ] two" })
    n.save(buf)
    local below = io.open(old):read("*a")
    -- Deleted: the save puts it back.
    vim.api.nvim_buf_set_lines(buf, -2, -1, false, {})
    n.save(buf)
    local deleted = io.open(old):read("*a")
    -- Another note's text pasted at its end, that note's ID line with it.
    vim.api.nvim_buf_set_lines(buf, -1, -1, false, { "# Other", "", "- [ ] pasted", "", "<!-- note-id: n-bbbbbbbbbbbb -->" })
    n.save(buf)
    local pasted = io.open(old):read("*a")
    -- A copy of the note's file, changed later, holds the same ID until opened.
    local copy = s.root() .. "/Old copy.md"
    local g = io.open(copy, "w"); g:write((deleted:gsub("# Old", "# Old copy"))); g:close()
    n.open(copy)
    local copied = io.open(copy):read("*a")
    io.write(vim.json.encode({
      made = made_text, id = id, pasted = pasted, given = given, kept_time = kept_time, concealed = concealed, below = below, deleted = deleted,
      copied = ids.in_text(copied), original = ids.of_file(old),
      title = s.title_of("\\n<!-- note-id: n-0123456789ab -->\\n"), preview = s.preview_of("# T\\n\\nwords\\n\\n<!-- note-id: n-0123456789ab -->\\n"),
      found = #s.search("note"),
    }))
  `);
  assert.match(result.made, /^# Fresh\n\n<!-- note-id: n-[0-9a-f]{12} -->\n$/, "a new note is made with its ID");
  assert.match(result.id ?? "", /^n-[0-9a-f]{12}$/);
  assert.equal(result.given, `# Old\n\n- [ ] one\n\n<!-- note-id: ${result.id} -->\n`, "an old note gets one when first opened");
  assert.equal(result.kept_time, 1767225600, "without its modified time changing, so it doesn't move up the list");
  assert.equal(result.concealed, true, "the line is hidden");
  assert.equal(result.below, `# Old\n\n- [ ] one\n\n- [ ] two\n\n<!-- note-id: ${result.id} -->\n`, "a line typed below it moves it last, every other line left as it is");
  assert.equal(result.deleted, result.below, "deleted, it comes back on save");
  assert.equal(result.pasted, `# Old\n\n- [ ] one\n\n- [ ] two\n\n# Other\n\n- [ ] pasted\n\n<!-- note-id: ${result.id} -->\n`, "another note's ID pasted in is dropped, and the note keeps its own");
  assert.equal(result.original, result.id, "the original keeps its ID");
  assert.ok(result.copied && result.copied !== result.id, "a copy gets an ID of its own");
  assert.equal(result.title, "New Note", "the ID line is never a title");
  assert.equal(result.preview, "words", "nor in a preview");
  assert.equal(result.found, 0, "nor found by a search");
  assert.ok(fs.existsSync(path.join(dir, "Old copy.md")));
});

test("a note's ID line is left out of the Format menu's paragraph styles, loses formatting wrapped round it on save, and stays hidden inside an unclosed code block", () => {
  const { result } = lua(`
    local s = require("notes.store")
    local n = require("notes")
    local path = s.create("", "Tasks")
    vim.cmd("Notes")
    n.open(path)
    local buf = vim.api.nvim_get_current_buf()
    vim.api.nvim_buf_set_lines(buf, 1, 2, false, { "one", "two" })
    local count = vim.api.nvim_buf_line_count(buf)
    require("notes.format").paragraph(buf, 2, count, "check")
    local checked = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    -- Formatting wrapped round the ID some other way: the save takes it out.
    vim.api.nvim_buf_set_lines(buf, count - 1, count, false, { "**" .. vim.api.nvim_buf_get_lines(buf, count - 1, count, false)[1] .. "**" })
    n.save(buf)
    local saved = io.open(path):read("*a")
    -- An unclosed code block above it.
    vim.api.nvim_buf_set_lines(buf, 1, 1, false, { "\`\`\`" })
    local kinds = require("notes.render").kinds(vim.api.nvim_buf_get_lines(buf, 0, -1, false))
    io.write(vim.json.encode({ checked = checked, saved = saved, last = kinds[#kinds].kind }))
  `);
  assert.deepEqual(result.checked.slice(1, 3), ["- [ ] one", "- [ ] two"]);
  assert.match(result.checked.at(-1), /^<!-- note-id: n-[0-9a-f]{12} -->$/, "the ID line isn't made a checklist item");
  assert.match(result.saved, /^# Tasks\n- \[ \] one\n- \[ \] two\n(- \[ \] \n)?\n<!-- note-id: n-[0-9a-f]{12} -->\n$/, "nor kept wrapped in bold");
  assert.doesNotMatch(result.saved, /\*\*/);
  assert.equal(result.last, "id", "hidden inside an unclosed code block");
});

test("renaming a new, empty note gives its first line the title, never its hidden ID line", () => {
  const { result } = lua(`
    local s = require("notes.store")
    local n = require("notes")
    local path = s.create("", nil)
    vim.cmd("Notes")
    local to = n.retitle(path, "Named")
    io.write(vim.json.encode({ text = io.open(to):read("*a") }))
  `);
  assert.match(result.text, /^# Named\n/, "the title on the first line");
  assert.equal(result.text.match(/note-id/g).length, 1, "and the ID line kept, once");
  assert.match(result.text, /\n<!-- note-id: n-[0-9a-f]{12} -->\n$/);
});
