-- Notes: the macOS Notes app's notes, in Neovim. `:Notes` (or `bin/notes`)
-- opens the sidebar of folders, tags and notes on the left and a note on the
-- right; the note is a Markdown file drawn the way Notes shows it (see
-- render.lua) and edited with vim. Notes save themselves as you go, and a
-- note's file is renamed when its title changes (see store.lua).
--
-- In a note:
--   Ctrl-F             find a note with fzf (in the sidebar too)
--   Enter              tick a checklist item, fold a heading, follow a link
--   a click            the same, on a ☐, a heading's ▾ or a link
--   Ctrl-P             the Format menu, with everything below
--   <leader>b i U x h `  bold, italic, underline, strikethrough, highlight, monospaced
--   <leader>1 2 3 0    title, heading, subheading, body
--   <leader>l 8 - 7 q  checklist, bulleted, dashed, numbered, block quote
--   <leader>m          monospaced block       <leader>t  table
--   <leader>k          link                   [[         link to a note
-- In Insert mode, Enter carries a list on, and Tab and Shift-Tab indent an
-- item or move between a table's cells.

local M = {}

local store = require("notes.store")
local render = require("notes.render")
local sidebar = require("notes.sidebar")
local format = require("notes.format")
local fold = require("notes.fold")
local links = require("notes.links")
local tables = require("notes.table")
local sync = require("notes.sync")

M.options = {
  -- Notes' "Move checked items to bottom", on so a list leads with what is
  -- left to do.
  move_checked_to_bottom = true,
  -- A click on a link follows it, as in Notes; off, a click only moves the
  -- cursor, and Enter follows.
  follow_links_on_click = true,
  -- How long after the last change a note is saved.
  autosave_ms = 800,
  -- Line numbers in the note window, relative to the cursor's line (which
  -- shows its own), for jumps like 5j; off, the note is bare, as in Notes.
  relative_line_numbers = true,
  -- A ticked item and everything indented under it struck through.
  strike_checked = true,
  -- A blank line between the items left to do and the ticked ones.
  gap_before_checked = true,
  -- The model that merges notes squashed in the sidebar (s marks, S squashes).
  squash_model = "sonnet",
}

function M.setup(opts)
  M.options = vim.tbl_extend("force", M.options, opts or {})
  format.options.move_checked_to_bottom = M.options.move_checked_to_bottom
  format.options.gap_before_checked = M.options.gap_before_checked
  render.options.strike_checked = M.options.strike_checked
  require("notes.squash").options.model = M.options.squash_model
end

-- ── Notes and their buffers ────────────────────────────────────────────────

local function is_note(buf)
  return vim.api.nvim_buf_is_valid(buf) and vim.b[buf].notes == true
end

local function note_buffers()
  return vim.tbl_filter(is_note, vim.api.nvim_list_bufs())
end

function M.current_path()
  if M.note_win and vim.api.nvim_win_is_valid(M.note_win) then
    local buf = vim.api.nvim_win_get_buf(M.note_win)
    if is_note(buf) then return vim.api.nvim_buf_get_name(buf) end
  end
  return nil
end

local function buffer_for(path)
  for _, buf in ipairs(note_buffers()) do
    if vim.fs.normalize(vim.api.nvim_buf_get_name(buf)) == vim.fs.normalize(path) then return buf end
  end
  return nil
end

-- Points a buffer at a note's new path, dropping the buffer Neovim keeps
-- under the old name.
local function rename_buffer(buf, to)
  local from = vim.api.nvim_buf_get_name(buf)
  -- A note moved with nothing changed in it keeps its time, which says when
  -- its text last changed (the squash goes by it).
  local unchanged = not vim.bo[buf].modified and vim.uv.fs_stat(to)
  vim.api.nvim_buf_set_name(buf, to)
  vim.api.nvim_buf_call(buf, function() vim.cmd("silent! keepalt write!") end)
  if unchanged then vim.uv.fs_utime(to, unchanged.atime.sec, unchanged.mtime.sec) end
  sync.written(buf)
  for _, other in ipairs(vim.api.nvim_list_bufs()) do
    if other ~= buf and vim.api.nvim_buf_get_name(other) == from and not vim.api.nvim_buf_is_loaded(other) then
      pcall(vim.api.nvim_buf_delete, other, { force = true })
    end
  end
end

-- A note changed from outside, now in its buffer (see sync.lua). An item
-- ticked there, by a script or Claude, moves under the ones left to do, as
-- ticking it here does, and the note is saved that way.
local function redraw(buf, rows)
  if format.options.move_checked_to_bottom and rows and #rows > 0 then
    local tick = vim.api.nvim_buf_get_changedtick(buf)
    table.sort(rows, function(a, b) return a > b end)
    for _, row in ipairs(rows) do format.sort_checked(buf, row) end
    if vim.api.nvim_buf_get_changedtick(buf) ~= tick then vim.schedule(function() M.save(buf) end) end
  end
  render.draw(buf)
end

-- Saves a note, and renames its file if its title changed. A change made to
-- its file from outside is brought in first, so it isn't written over.
function M.save(buf)
  if not is_note(buf) then return end
  if not sync.check(buf, redraw) then return end
  local path = vim.api.nvim_buf_get_name(buf)
  if vim.bo[buf].modified then
    vim.api.nvim_buf_call(buf, function() vim.cmd("silent! keepalt write!") end)
    sync.written(buf)
  end
  if store.in_trash(path) then return end
  local renamed = store.rename_to_title(path)
  if renamed ~= path then rename_buffer(buf, renamed) end
  if sidebar.window() then sidebar.draw() end
end

function M.save_all()
  for _, buf in ipairs(note_buffers()) do M.save(buf) end
end

-- A note deleted from the sidebar: its buffer goes, and the note window
-- shows the next note, or an empty one.
function M.forget(path)
  local buf = buffer_for(path)
  if not buf then return end
  M.save(buf)
  if M.note_win and vim.api.nvim_win_is_valid(M.note_win) and vim.api.nvim_win_get_buf(M.note_win) == buf then
    local next_note
    for _, note in ipairs(sidebar.view_notes()) do
      if note.path ~= vim.fs.normalize(path) then
        next_note = note.path
        break
      end
    end
    if next_note then
      M.open(next_note, { stay = true })
    else
      vim.api.nvim_win_set_buf(M.note_win, vim.api.nvim_create_buf(false, true))
    end
  end
  pcall(vim.api.nvim_buf_delete, buf, { force = true })
end

function M.moved(from, to)
  local buf = buffer_for(from)
  if buf then rename_buffer(buf, to) end
end

function M.folder_renamed(from, to)
  local root_from, root_to = store.absolute(from) .. "/", store.absolute(to) .. "/"
  for _, buf in ipairs(note_buffers()) do
    local name = vim.api.nvim_buf_get_name(buf)
    if name:sub(1, #root_from) == root_from then rename_buffer(buf, root_to .. name:sub(#root_from + 1)) end
  end
end

-- ── Opening ────────────────────────────────────────────────────────────────

local function setup_window(win)
  local wo = vim.wo[win]
  wo.conceallevel = 2
  wo.concealcursor = "nc"
  wo.wrap = true
  wo.linebreak = true
  wo.breakindent = true
  wo.number = M.options.relative_line_numbers
  wo.relativenumber = M.options.relative_line_numbers
  wo.signcolumn = "no"
  wo.list = false
  wo.cursorline = false
  wo.spell = false
  wo.showbreak = "NONE"
  fold.attach(win)
end

-- Whatever Enter or a click does at a place in a note: tick a checklist item,
-- fold a heading, follow a link. `col` is a 0-based byte column; `clicked`
-- limits it to the ☐ and the ▾ themselves. Returns whether it did something.
function M.act(buf, row, col, clicked)
  if vim.fn.foldclosed(row) ~= -1 then
    vim.cmd(row .. "foldopen")
    return true
  end
  local kinds = render.kinds(vim.api.nvim_buf_get_lines(buf, 0, -1, false))
  local k = kinds[row]
  if k.kind == "check" and (not clicked or (col >= k.indent and col < k.indent + k.marker)) then
    return format.toggle_check(buf, row)
  end
  if (k.kind == "title" or k.kind == "heading" or k.kind == "subheading") and (not clicked or (k.marker > 0 and col < k.marker)) then
    return fold.toggle(buf, row)
  end
  if not clicked or M.options.follow_links_on_click then return links.follow(buf, row, col) end
  return false
end

local function feed(keys)
  vim.api.nvim_feedkeys(vim.api.nvim_replace_termcodes(keys, true, false, true), "n", false)
end

-- The Format menu: everything a note can do, each with its key, in a small
-- window. A key, Enter or a click on a line picks it; Escape or q closes it.
-- Opened from Visual mode, it acts on the selection. A paragraph or character
-- style (those marked `typing`) leaves you typing, as `a` would, unless it
-- already has: you pick a style to write in it.
function M.menu(buf, visual, typing_col)
  local first, last = vim.fn.line("."), vim.fn.line(".")
  if visual then first, last = vim.fn.line("'<"), vim.fn.line("'>") end
  local function para(style) return { fn = function() format.paragraph(buf, first, last, style) end, typing = true } end
  local function char(style) return { fn = function() format.character(buf, style, visual, typing_col) end, typing = true } end
  local items = {
    { "Paragraph" },
    { "1", "Title", para("title") },
    { "2", "Heading", para("heading") },
    { "3", "Subheading", para("subheading") },
    { "0", "Body", para("body") },
    { "m", "Monospaced", { fn = function() format.monospaced(buf, first, last) end, typing = true } },
    { "8", "Bulleted List", para("bullet") },
    { "-", "Dashed List", para("dash") },
    { "7", "Numbered List", para("number") },
    { "q", "Block Quote", para("quote") },
    { "Character" },
    { "b", "Bold", char("bold") },
    { "i", "Italic", char("italic") },
    { "U", "Underline", char("underline") },
    { "x", "Strikethrough", char("strike") },
    { "h", "Highlight", char("highlight") },
    { "`", "Monospaced text", char("code") },
    { "k", "Link…", function() format.link(buf, visual) end },
    { "Checklist" },
    { "l", "Checklist", para("check") },
    { "c", "Tick or untick", function() format.toggle_check(buf, first) end },
    { "s", "Move checked to bottom", function() format.sort_checked(buf, first) end },
    { "u", "Uncheck all", function() format.uncheck_all(buf) end },
    { "D", "Delete checked items", function() format.delete_checked(buf) end },
    { "Table" },
    { "t", "Table", function() tables.insert(buf) end },
    { "r", "Add row", function() tables.add_row(buf, first) end },
    { "C", "Add column", function() tables.add_column(buf) end },
    { "R", "Delete row", function() tables.delete_row(buf) end },
    { "X", "Delete column", function() tables.delete_column(buf) end },
  }
  local lines, actions = {}, {}
  for _, item in ipairs(items) do
    if #item == 1 then
      table.insert(lines, (#lines > 0 and "" or "") .. " " .. item[1])
    else
      table.insert(lines, string.format("   %-3s %s", item[1], item[2]))
      actions[#lines] = item[3]
    end
  end
  local menu_buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(menu_buf, 0, -1, false, lines)
  vim.bo[menu_buf].modifiable = false
  local ns = vim.api.nvim_create_namespace("notes.menu")
  for row, line in ipairs(lines) do
    if not actions[row] then vim.api.nvim_buf_set_extmark(menu_buf, ns, row - 1, 0, { end_col = #line, hl_group = "Title" }) end
  end
  local origin = vim.api.nvim_get_current_win()
  local win = vim.api.nvim_open_win(menu_buf, true, {
    relative = "cursor", row = 1, col = 0, width = 30, height = math.min(#lines, vim.o.lines - 4),
    style = "minimal", border = "rounded", title = " Format ", title_pos = "center",
  })
  local function pick(action)
    vim.api.nvim_win_close(win, true)
    vim.api.nvim_set_current_win(origin)
    if not action then return end
    if type(action) == "function" then return action() end
    action.fn()
    -- Through the note's own `a` (mapped, so it starts after a line's
    -- marker), once the style is in; not over a selection, which you were
    -- formatting rather than writing.
    if action.typing and not visual then
      vim.schedule(function()
        if vim.api.nvim_get_mode().mode ~= "i" then vim.api.nvim_feedkeys("a", "m", false) end
      end)
    end
  end
  local opts = { buffer = menu_buf, nowait = true, silent = true }
  for row, item in pairs(actions) do
    local key = vim.trim(lines[row]):match("^(%S+)")
    vim.keymap.set("n", key, function() pick(item) end, opts)
  end
  vim.keymap.set("n", "<CR>", function() pick(actions[vim.fn.line(".")]) end, opts)
  vim.keymap.set("n", "<LeftRelease>", function()
    local pos = vim.fn.getmousepos()
    if pos.winid == win then pick(actions[pos.line]) else pick(nil) end
  end, opts)
  vim.keymap.set("n", "<Esc>", function() pick(nil) end, opts)
  vim.keymap.set("n", "Q", function() pick(nil) end, opts)
end

local function setup_buffer(buf)
  if vim.b[buf].notes_ready then return end
  vim.b[buf].notes = true
  vim.b[buf].notes_ready = true
  sync.track(buf)
  vim.bo[buf].filetype = "notes"
  vim.bo[buf].swapfile = false
  -- No undo history on disk either, so a deleted note's text doesn't stay
  -- in ~/.local/state/nvim/undo.
  vim.bo[buf].undofile = false
  vim.bo[buf].textwidth = 0
  -- Notes stay on this Mac: no Copilot suggestions, which would send them to
  -- GitHub, and no completion popup but the one for [[ links: nvim-cmp isn't
  -- loaded in the notes app at all (nvim/lua/plugins/lsp.lua), since it set
  -- its own Ctrl-P on a note each time typing started, over the Format menu's.
  vim.b[buf].copilot_enabled = false
  render.attach(buf)

  local map = function(modes, lhs, fn, desc) vim.keymap.set(modes, lhs, fn, { buffer = buf, silent = true, desc = desc }) end
  map("n", "<CR>", function()
    local row, col = unpack(vim.api.nvim_win_get_cursor(0))
    if not M.act(buf, row, col, false) then feed("<CR>") end
  end, "Tick, fold or follow")
  for _, mode in ipairs({ "n", "i" }) do
    map(mode, "<LeftRelease>", function()
      local pos = vim.fn.getmousepos()
      if pos.winid == vim.api.nvim_get_current_win() and pos.line > 0 then M.act(buf, pos.line, pos.column - 1, true) end
    end, "Tick, fold or follow what was clicked")
  end
  -- Ctrl-P, which outside a note is your config's Telescope file finder.
  map("n", "<C-p>", function() M.menu(buf, false) end, "Format menu")
  map("x", "<C-p>", function()
    feed("<Esc>")
    vim.schedule(function() M.menu(buf, true) end)
  end, "Format menu")
  -- i, a and I start typing after a line's marker, never inside it.
  for _, key in ipairs({ "i", "a", "I" }) do
    map("n", key, function()
      local row, col = unpack(vim.api.nvim_win_get_cursor(0))
      local at = format.insert_point(vim.api.nvim_get_current_line(), col, key)
      if not at then return feed(key) end
      vim.api.nvim_win_set_cursor(0, { row, at })
      vim.cmd(at >= #vim.api.nvim_get_current_line() and "startinsert!" or "startinsert")
    end, "Insert after the line's marker")
  end
  -- In Insert mode too, where Ctrl-P would be vim's word completion: it leaves
  -- Insert mode and acts on the word or line you were typing in.
  map("i", "<C-p>", function()
    -- Where you were typing, which leaving Insert mode moves the cursor off.
    local typing_col = vim.api.nvim_win_get_cursor(0)[2]
    vim.cmd("stopinsert")
    vim.schedule(function() M.menu(buf, false, typing_col) end)
  end, "Format menu")
  -- Ctrl-F, in place of vim's page down: find a note (see finder.lua).
  map("n", "<C-f>", function() require("notes.finder").open() end, "Find a note")
  map("i", "<C-f>", function()
    vim.cmd("stopinsert")
    vim.schedule(function() require("notes.finder").open() end)
  end, "Find a note")
  -- An empty pair of style markers put in to type into goes if nothing was.
  vim.api.nvim_create_autocmd("InsertLeave", { buffer = buf, callback = function() format.drop_empty(buf) end })
  -- Character styles, on the word under the cursor or the selection.
  for key, style in pairs({ b = "bold", i = "italic", U = "underline", x = "strike", h = "highlight", ["`"] = "code" }) do
    map("n", "<leader>" .. key, function() format.character(buf, style, false) end, style)
    map("x", "<leader>" .. key, function()
      feed("<Esc>")
      vim.schedule(function() format.character(buf, style, true) end)
    end, style)
  end
  -- Paragraph styles, on the line or every line selected.
  for key, style in pairs({ ["1"] = "title", ["2"] = "heading", ["3"] = "subheading", ["0"] = "body", l = "check", ["8"] = "bullet", ["-"] = "dash", ["7"] = "number", q = "quote" }) do
    map("n", "<leader>" .. key, function() format.paragraph(buf, vim.fn.line("."), vim.fn.line("."), style) end, style)
    map("x", "<leader>" .. key, function()
      feed("<Esc>")
      vim.schedule(function() format.paragraph(buf, vim.fn.line("'<"), vim.fn.line("'>"), style) end)
    end, style)
  end
  map("n", "<leader>m", function() format.monospaced(buf, vim.fn.line("."), vim.fn.line(".")) end, "Monospaced block")
  map("x", "<leader>m", function()
    feed("<Esc>")
    vim.schedule(function() format.monospaced(buf, vim.fn.line("'<"), vim.fn.line("'>")) end)
  end, "Monospaced block")
  map("n", "<leader>t", function() tables.insert(buf) end, "Table")
  map("n", "<leader>k", function() format.link(buf, false) end, "Link")
  map("x", "<leader>k", function()
    feed("<Esc>")
    vim.schedule(function() format.link(buf, true) end)
  end, "Link")

  map("i", "<CR>", function()
    if vim.fn.pumvisible() == 1 then return feed("<C-y>") end
    if not format.enter(buf) then feed("<CR>") end
  end, "Carry the list on")
  map("i", "<Tab>", function()
    if not format.tab(buf, 1) then feed("<Tab>") end
  end, "Next cell, or indent the item")
  map("i", "<S-Tab>", function()
    if not format.tab(buf, -1) then feed("<S-Tab>") end
  end, "Previous cell, or outdent the item")

  -- Saved a moment after each change, and whenever you leave the note.
  local timer = vim.uv.new_timer()
  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI", "InsertLeave" }, {
    buffer = buf,
    callback = function()
      timer:stop()
      timer:start(M.options.autosave_ms, 0, vim.schedule_wrap(function() M.save(buf) end))
    end,
  })
  vim.api.nvim_create_autocmd({ "BufLeave", "FocusLost" }, { buffer = buf, callback = function() M.save(buf) end })
  -- Tables line up again after each edit, and [[ offers note titles.
  vim.api.nvim_create_autocmd({ "InsertLeave", "TextChanged" }, {
    buffer = buf,
    callback = function()
      if vim.api.nvim_get_current_buf() == buf then tables.align(buf, vim.fn.line(".")) end
    end,
  })
  vim.api.nvim_create_autocmd("TextChangedI", { buffer = buf, callback = links.complete })
  vim.api.nvim_create_autocmd("BufWipeout", { buffer = buf, callback = function() timer:close() end })
end

-- Opens a note in the note window. With insert_title, the cursor goes to the
-- end of its first line in Insert mode, for a new note's title; with stay,
-- the cursor stays where it is.
function M.open(path, opts)
  opts = opts or {}
  if not (M.note_win and vim.api.nvim_win_is_valid(M.note_win)) then M.layout() end
  local current = vim.api.nvim_win_get_buf(M.note_win)
  if is_note(current) then M.save(current) end
  -- The buffer is made and marked Copilot-off before it is loaded, so a
  -- plugin that attaches when the file's type is set (Copilot does, on
  -- FileType) already sees the mark.
  local buf = vim.fn.bufadd(path)
  vim.b[buf].copilot_enabled = false
  vim.api.nvim_win_call(M.note_win, function() vim.cmd("silent keepalt buffer " .. buf) end)
  store.set_last(store.relative(path))
  setup_buffer(buf)
  setup_window(M.note_win)
  tables.align_all(buf)
  render.draw(buf)
  sidebar.draw()
  -- Once what opened it is done: a note opened in place of one deleted,
  -- restored or squashed is found after the list has dropped that one.
  -- Only for the note still open then: one opened on the way (forget
  -- opening the next while several go) is passed over.
  vim.schedule(function()
    local now = M.current_path()
    if now and vim.fs.normalize(now) == vim.fs.normalize(path) then sidebar.reveal(path) end
  end)
  if not opts.stay then vim.api.nvim_set_current_win(M.note_win) end
  if opts.insert_title then
    vim.api.nvim_win_set_cursor(M.note_win, { 1, #vim.api.nvim_buf_get_lines(buf, 0, 1, false)[1] })
    vim.cmd("startinsert!")
  end
end

function M.focus_note()
  if M.note_win and vim.api.nvim_win_is_valid(M.note_win) then vim.api.nvim_set_current_win(M.note_win) end
end

-- The sidebar shown or put away: tmux/notes-open.sh puts it away while Notes
-- sits beside another tab's pane (M-e), where the note wants the room, and
-- shows it again in Notes' own tab. NOTES_SIDEBAR=off starts the app without it.
function M.show_sidebar(show)
  local win = sidebar.window()
  if show then
    if not win then
      M.layout()
      sidebar.reveal(M.current_path())
    end
  elseif win and #vim.api.nvim_tabpage_list_wins(0) > 1 then
    vim.api.nvim_win_close(win, true)
    M.focus_note()
  end
end

function M.quit()
  M.save_all()
  vim.cmd("qa")
end

-- The sidebar on the left, the note window the rest.
function M.layout()
  if not (M.note_win and vim.api.nvim_win_is_valid(M.note_win)) then M.note_win = vim.api.nvim_get_current_win() end
  if not sidebar.window() then
    vim.api.nvim_win_call(M.note_win, function()
      vim.cmd("topleft vertical " .. sidebar.WIDTH .. "split")
      local win = vim.api.nvim_get_current_win()
      vim.api.nvim_win_set_buf(win, sidebar.buffer())
      local wo = vim.wo[win]
      wo.winfixwidth = true
      wo.number = false
      wo.relativenumber = false
      wo.signcolumn = "no"
      wo.cursorline = true
      wo.wrap = false
      wo.foldenable = false
      wo.list = false
      wo.fillchars = "eob: "
    end)
  end
  M.fit_sidebar()
  sidebar.draw()
end

-- The sidebar's width for the screen: its full width wherever the note keeps
-- room beside it, else narrower, but never under MIN_WIDTH. Neovim shrinks a
-- window when its pane gets narrow (M-e moving Notes beside a pane) and never
-- grows it back, so this runs on every resize.
M.MIN_WIDTH = 24
function M.fit_sidebar()
  local win = sidebar.window()
  if not win then return end
  local width = math.max(M.MIN_WIDTH, math.min(sidebar.WIDTH, vim.o.columns - 60))
  if vim.api.nvim_win_get_width(win) ~= width then
    vim.api.nvim_win_set_width(win, width)
    sidebar.draw()
  end
end

M.WELCOME = [==[
# Welcome to Notes

This note shows what a note can do. Change it, or delete it from the list on the left with `dd`. Press Ctrl-P for the Format menu.

## Formatting

**Bold**, *italic*, <u>underlined</u>, ~~struck through~~, ==highlighted== and `monospaced` text, each a `<leader>` key away: b, i, U, x, h and `.

### Lists

* A bulleted list
- A dashed list
1. A numbered list
2. Enter carries a list on, and Tab indents an item

### Checklist

- [ ] Click a box, or press Enter on its line, to tick it
- [x] Ticked
- [ ] Press Enter on a heading to fold its section

> A block quote, for what someone said.

```
A monospaced block, for code or anything that needs its spacing kept.
```

| Table | Tab moves between cells |
| ----- | ----------------------- |
| Rows  | keep themselves lined up |

## Links

A web link: https://www.apple.com/macos/, a [link with text](https://neovim.io), a link to a note: [[Welcome to Notes]], and a #tag.
]==]

-- Another Notes has the notes open (see store.lock): this one says so, and
-- q closes it, rather than start and write over that one's notes.
local function already_open(pid)
  local buf = vim.api.nvim_get_current_buf()
  vim.bo[buf].buftype = "nofile"
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, {
    "",
    "  Notes is open already, in another Neovim (process " .. pid .. ").",
    "",
    "  Two of them would write over each other's notes as you type, so this",
    "  one hasn't opened them. Use that one, or quit it and open Notes again.",
    "",
    "  Press q to close this.",
  })
  vim.bo[buf].modifiable = false
  vim.keymap.set("n", "q", "<Cmd>qa<CR>", { buffer = buf, nowait = true })
end

function M.start()
  local locked, holder = store.lock()
  if not locked then return already_open(holder) end
  vim.api.nvim_create_autocmd("VimLeavePre", { callback = store.unlock })
  -- Files changed from outside reach their open notes, and the sidebar.
  local unwatch = sync.watch(store.root(), function()
    for _, buf in ipairs(note_buffers()) do sync.check(buf, redraw) end
    if sidebar.window() then sidebar.draw() end
  end)
  vim.api.nvim_create_autocmd("VimLeavePre", { callback = unwatch })
  render.highlights()
  sidebar.highlights()
  vim.api.nvim_create_autocmd("ColorScheme", {
    callback = function()
      render.highlights()
      sidebar.highlights()
    end,
  })
  vim.o.mouse = "a"
  M.note_win = vim.api.nvim_get_current_win()
  M.layout()
  local notes = store.notes()
  if #notes == 0 then
    local path = store.create("", "Welcome to Notes")
    local file = assert(io.open(path, "w"))
    file:write(M.WELCOME)
    file:close()
    notes = store.notes()
  end
  -- The note you had open last, or the newest.
  M.open(store.last() or notes[1].path)
  if vim.env.NOTES_SIDEBAR == "off" then M.show_sidebar(false) end
  vim.api.nvim_create_autocmd("VimLeavePre", { callback = M.save_all })
  vim.api.nvim_create_autocmd("VimResized", { callback = M.fit_sidebar })
end

return M
