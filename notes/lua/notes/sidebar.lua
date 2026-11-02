-- The list on the left, as in Notes: folders, tags, then the notes in what is
-- chosen, pinned ones first, then newest first, each as its title and a
-- line of when it changed and how it starts. A click or Enter on a folder or
-- a tag shows its notes; on a note, opens it.
--
--   n  new note here          N  new folder        dd  delete (to Recently Deleted)
--   p  pin or unpin           m  move to a folder  r   restore, in Recently Deleted
--   /  search all notes       x  clear the search or tag   Ctrl-F  find a note (fzf)
--   R  rename a folder        q  quit              ?   these keys

local M = {}

local store = require("notes.store")

M.WIDTH = 34
M.ns = vim.api.nvim_create_namespace("notes.sidebar")

-- What is shown: a folder (nil for All Notes, "" for the top folder,
-- store.TRASH for Recently Deleted), or the notes with a tag, or those
-- matching a search.
-- marked: the notes marked with s for squashing, in the order marked.
M.state = { folder = nil, tag = nil, query = nil, items = {}, marked = {} }

local function when(mtime)
  local now = os.time()
  if os.date("%Y%m%d", mtime) == os.date("%Y%m%d", now) then return os.date("%-I:%M %p", mtime) end
  if os.date("%Y%m%d", mtime) == os.date("%Y%m%d", now - 86400) then return "Yesterday" end
  if now - mtime < 6 * 86400 then return os.date("%A", mtime) end
  return os.date("%-m/%-d/%y", mtime)
end

local function fit(text, width)
  text = text:gsub("[\n\t]", " ")
  if vim.fn.strdisplaywidth(text) <= width then return text end
  return vim.fn.strcharpart(text, 0, width - 1) .. "…"
end

-- A row's name on the left and a number on the right.
local function counted(name, count, width)
  local number = tostring(count)
  local room = width - #number - 1
  return fit(name, room) .. string.rep(" ", math.max(room - vim.fn.strdisplaywidth(fit(name, room)), 0) + 1) .. number
end

function M.view_title()
  local s = M.state
  if s.query then return "Search: " .. s.query end
  if s.tag then return "#" .. s.tag end
  if s.folder == nil then return "All Notes" end
  if s.folder == "" then return "Notes" end
  if s.folder == store.TRASH then return "Recently Deleted" end
  return vim.fs.basename(s.folder)
end

function M.view_notes()
  local s = M.state
  if s.query then return store.search(s.query) end
  if s.tag then return store.tagged(s.tag) end
  if s.folder == "" then
    return vim.tbl_filter(function(note) return note.folder == "" end, store.notes())
  end
  return store.notes(s.folder)
end

-- The sidebar's lines, highlights and what each line does, for `draw`.
function M.build(current_path)
  local width = M.WIDTH - 2
  local lines, marks, items = {}, {}, {}
  local function add(text, group, item)
    table.insert(lines, " " .. text)
    if group then table.insert(marks, { #lines - 1, group }) end
    items[#lines] = item
  end
  local all = store.notes()
  local s = M.state
  local plain = not s.tag and not s.query
  add("Folders", "NotesSidebarHeader")
  add(counted("  All Notes", #all, width), plain and s.folder == nil and "NotesSidebarCurrent" or nil, { folder = nil, all = true })
  local top = #vim.tbl_filter(function(note) return note.folder == "" end, all)
  add(counted("  Notes", top, width), plain and s.folder == "" and "NotesSidebarCurrent" or nil, { folder = "" })
  for _, folder in ipairs(store.folders()) do
    add(counted(string.rep("  ", folder.depth + 2) .. folder.name, folder.count, width), plain and s.folder == folder.rel and "NotesSidebarCurrent" or nil, { folder = folder.rel })
  end
  add(counted("  Recently Deleted", #store.notes(store.TRASH), width), plain and s.folder == store.TRASH and "NotesSidebarCurrent" or nil, { folder = store.TRASH })
  local tags = store.tags()
  if #tags > 0 then
    add("")
    add("Tags", "NotesSidebarHeader")
    for _, tag in ipairs(tags) do
      add(counted("  #" .. tag.name, tag.count, width), s.tag and s.tag:lower() == tag.name:lower() and "NotesSidebarCurrent" or "NotesTag", { tag = tag.name })
    end
  end
  add("")
  add(string.rep("─", width), "NotesSidebarRule")
  local notes = M.view_notes()
  add(fit(M.view_title(), width - 8) .. string.rep(" ", math.max(width - 8 - vim.fn.strdisplaywidth(fit(M.view_title(), width - 8)), 0)) .. string.format("%8s", #notes .. (#notes == 1 and " note" or " notes")), "NotesSidebarHeader")
  if #notes == 0 then
    add("")
    add(s.query and "  No notes match." or "  No notes. Press n to make one.", "Comment")
  end
  local section
  for _, note in ipairs(notes) do
    local this = note.pinned and "Pinned" or "Notes"
    if this ~= section and (note.pinned or section == "Pinned") then
      add("")
      add(this, "NotesSidebarSection")
    elseif section == nil then
      add("")
    end
    section = this
    local item = { note = note.path }
    local open = current_path and vim.fs.normalize(current_path) == note.path
    local marked = vim.tbl_contains(s.marked, note.path)
    add(fit((marked and "✓ " or note.pinned and "● " or "  ") .. note.title, width), open and "NotesSidebarOpen" or "NotesSidebarTitle", item)
    local detail = s.query and note.match or note.preview
    if s.folder == nil and not s.query and not s.tag and note.folder ~= "" then detail = "▸ " .. note.folder .. "  " .. detail end
    add(fit("  " .. when(note.mtime) .. "  " .. detail, width), "NotesSidebarDetail", item)
    add("", nil, nil)
  end
  return lines, marks, items
end

function M.highlights()
  local set = function(name, spec) vim.api.nvim_set_hl(0, name, spec) end
  set("NotesSidebarHeader", { link = "Title" })
  set("NotesSidebarSection", { link = "Special" })
  set("NotesSidebarCurrent", { link = "Visual" })
  set("NotesSidebarOpen", { link = "PmenuSel" })
  set("NotesSidebarTitle", { bold = true })
  set("NotesSidebarDetail", { link = "Comment" })
  set("NotesSidebarRule", { link = "WinSeparator" })
end

-- ── The buffer and window ──────────────────────────────────────────────────

function M.buffer()
  if M.buf and vim.api.nvim_buf_is_valid(M.buf) then return M.buf end
  M.buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_name(M.buf, "notes://sidebar")
  vim.bo[M.buf].buftype = "nofile"
  vim.bo[M.buf].bufhidden = "hide"
  vim.bo[M.buf].swapfile = false
  vim.bo[M.buf].filetype = "notes-sidebar"
  M.keys(M.buf)
  return M.buf
end

function M.window()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == M.buf then return win end
  end
  return nil
end

function M.draw()
  local buf = M.buffer()
  local current = require("notes").current_path()
  local lines, marks, items = M.build(current)
  M.state.items = items
  vim.bo[buf].modifiable = true
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(buf, M.ns, 0, -1)
  for _, m in ipairs(marks) do
    pcall(vim.api.nvim_buf_set_extmark, buf, M.ns, m[1], 0, { line_hl_group = (m[2] == "NotesSidebarCurrent" or m[2] == "NotesSidebarOpen") and m[2] or nil, hl_group = m[2], end_col = #lines[m[1] + 1] })
  end
end

-- Shows a folder, a tag or a search: { folder = … } | { tag = … } | { query = … }.
function M.show(view)
  M.state.folder, M.state.tag, M.state.query = view.folder, view.tag, view.query
  -- Put away beside a tab (show_sidebar), it comes back to show what was asked for.
  if not M.window() then require("notes").layout() end
  M.draw()
  local win = M.window()
  if win then
    for row, item in pairs(M.state.items) do
      if item.note then
        vim.api.nvim_win_set_cursor(win, { row, 0 })
        break
      end
    end
  end
end

-- ── Acting on a line ───────────────────────────────────────────────────────

local function item_at(row)
  return M.state.items[row or vim.api.nvim_win_get_cursor(0)[1]]
end

function M.activate(row)
  local item = item_at(row)
  if not item then return end
  if item.note then
    require("notes").open(item.note)
  elseif item.tag then
    M.show({ tag = item.tag })
  elseif item.folder ~= nil or item.all then
    M.show({ folder = item.folder })
  end
end

-- The folder a new note goes in: the one shown, or the top folder for All
-- Notes, a tag or a search.
local function here()
  local f = M.state.folder
  if M.state.tag or M.state.query or f == nil or f == store.TRASH then return "" end
  return f
end

function M.new_note()
  local path = store.create(here(), nil)
  M.draw()
  require("notes").open(path, { insert_title = true })
end

function M.new_folder()
  vim.ui.input({ prompt = "New folder: " }, function(name)
    if not name or vim.trim(name) == "" then return end
    local base = here()
    local rel = store.create_folder((base ~= "" and base .. "/" or "") .. name)
    if rel then M.show({ folder = rel }) end
  end)
end

function M.delete()
  local item = item_at()
  if not item or not item.note then return end
  local path = item.note
  local function go()
    require("notes").forget(path)
    store.delete(path)
    M.draw()
  end
  if store.in_trash(path) then
    vim.ui.select({ "Delete for good", "Keep it" }, { prompt = "Delete this note for good? It can't be undone." }, function(choice)
      if choice == "Delete for good" then go() end
    end)
  else
    go()
  end
end

-- t: the tab for the note under the cursor, the one linked to it, or a new
-- one from M-t's popup, so you pick its directory (tmux/note-tab.sh).
function M.tab()
  local item = item_at()
  if not item or not item.note or store.in_trash(item.note) then return end
  if (vim.env.TMUX or "") == "" then return vim.notify("A tab for a note needs tmux.") end
  vim.system({ vim.fn.expand("~/.config/tmux/note-tab.sh"), store.relative(item.note) }, { detach = true })
end

-- s: marks the note for squashing, or unmarks it.
function M.mark()
  local item = item_at()
  if not item or not item.note or store.in_trash(item.note) then return end
  local at = vim.tbl_contains(M.state.marked, item.note)
  if at then
    M.state.marked = vim.tbl_filter(function(p) return p ~= item.note end, M.state.marked)
  else
    table.insert(M.state.marked, item.note)
  end
  M.draw()
end

-- S: squashes the marked notes into one (squash.lua), after asking, since
-- their text goes to Claude to merge.
function M.squash()
  local marked = vim.tbl_filter(function(p) return vim.uv.fs_stat(p) ~= nil end, M.state.marked)
  if #marked < 2 then
    vim.notify("Mark two or more notes with s, then S squashes them into one.")
    return
  end
  local squash = require("notes.squash")
  vim.ui.select({ "Squash them", "Keep them as they are" }, {
    prompt = ("Squash %d notes into one? Their text goes to Claude (%s) to merge; the originals go to Recently Deleted."):format(#marked, squash.options.model),
  }, function(choice)
    if choice ~= "Squash them" then return end
    local titles = vim.tbl_map(function(p) return store.title_of(io.open(p):read("*a")) end, marked)
    local job, cancelled
    local close = squash.progress(titles, squash.options.model, function()
      cancelled = true
      if job then job:kill(15) end
      vim.notify("Squash canceled; the notes are as they were.")
    end)
    job = squash.run(marked, function(path, why)
      close()
      if cancelled then return end
      if not path then return vim.notify("Notes weren't squashed: " .. why, vim.log.levels.WARN) end
      M.state.marked = {}
      require("notes").open(path)
      M.draw()
      vim.notify("Squashed into " .. store.title_of(io.open(path):read("*a")))
    end)
  end)
end

function M.restore()
  local item = item_at()
  if not item or not item.note or not store.in_trash(item.note) then return end
  require("notes").forget(item.note)
  store.restore(item.note)
  M.draw()
end

function M.pin()
  local item = item_at()
  if not item or not item.note or store.in_trash(item.note) then return end
  local rel = store.relative(item.note)
  store.set_pinned(rel, not store.is_pinned(rel))
  M.draw()
end

function M.move()
  local item = item_at()
  if not item or not item.note then return end
  local choices = { { label = "Notes", rel = "" } }
  for _, folder in ipairs(store.folders()) do
    table.insert(choices, { label = string.rep("  ", folder.depth) .. folder.name, rel = folder.rel })
  end
  vim.ui.select(choices, { prompt = "Move to", format_item = function(c) return c.label end }, function(choice)
    if not choice then return end
    local path = item.note
    local moved = store.move(path, choice.rel)
    require("notes").moved(path, moved)
    M.draw()
  end)
end

function M.rename_folder()
  local item = item_at()
  if not item or not item.folder or item.folder == "" or item.folder == store.TRASH then return end
  vim.ui.input({ prompt = "Rename folder: ", default = vim.fs.basename(item.folder) }, function(name)
    if not name or vim.trim(name) == "" then return end
    require("notes").save_all()
    local to, why = store.rename_folder(item.folder, name)
    if not to then return vim.notify("notes: didn't rename the folder: " .. why, vim.log.levels.WARN) end
    require("notes").folder_renamed(item.folder, to)
    M.show({ folder = to })
  end)
end

function M.search()
  vim.ui.input({ prompt = "Search notes: ", default = M.state.query or "" }, function(query)
    if query == nil then return end
    if vim.trim(query) == "" then return M.show({ folder = M.state.folder }) end
    M.show({ query = vim.trim(query) })
  end)
end

function M.help()
  local text = {
    " Notes",
    "",
    "  Enter or click   open a note, folder or tag",
    "  n                new note in this folder",
    "  N                new folder",
    "  dd               delete (to Recently Deleted)",
    "  r                restore from Recently Deleted",
    "  p                pin or unpin",
    "  t                its tab: go there, or open one",
    "  s  S             mark notes, squash them into one",
    "  m                move to a folder",
    "  R                rename a folder",
    "  /                search every note",
    "  Ctrl-F           find a note with fzf",
    "  x                clear the search or tag",
    "  Tab              go to the note",
    "  q                quit",
    "",
    " In a note, Ctrl-P opens the Format menu.",
  }
  local buf = vim.api.nvim_create_buf(false, true)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, text)
  local win = vim.api.nvim_open_win(buf, true, { relative = "editor", style = "minimal", border = "rounded", width = 52, height = #text, row = 3, col = 6 })
  vim.keymap.set("n", "q", function() vim.api.nvim_win_close(win, true) end, { buffer = buf })
  vim.keymap.set("n", "<Esc>", function() vim.api.nvim_win_close(win, true) end, { buffer = buf })
end

function M.keys(buf)
  local map = function(lhs, fn) vim.keymap.set("n", lhs, fn, { buffer = buf, nowait = true, silent = true }) end
  map("<CR>", function() M.activate() end)
  map("<LeftRelease>", function()
    local pos = vim.fn.getmousepos()
    if pos.winid == vim.api.nvim_get_current_win() and pos.line > 0 then M.activate(pos.line) end
  end)
  map("n", M.new_note)
  map("N", M.new_folder)
  map("t", M.tab)
  map("s", M.mark)
  map("S", M.squash)
  map("dd", M.delete)
  map("D", M.delete)
  map("r", M.restore)
  map("p", M.pin)
  map("m", M.move)
  map("R", M.rename_folder)
  map("/", M.search)
  map("<C-f>", function() require("notes.finder").open() end)
  map("x", function() M.show({ folder = M.state.folder }) end)
  map("<Tab>", function() require("notes").focus_note() end)
  map("q", function() require("notes").quit() end)
  map("?", M.help)
end

return M
