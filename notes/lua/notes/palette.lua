-- Ctrl-Shift-P: the notes app's command palette, in place of the keymaps
-- palette Neovim's config gives it (Telescope's). Every command Notes has,
-- each with its key, in fzf in a floating window, as Ctrl-F's finder is:
-- the list's (on the note open, from a note), the Format menu's (from a
-- note), folding, the app's, and Switchboard's commands for Notes, which it
-- presses in tmux as their keys are. The ones picked last come first. The
-- notes_palette setting gives the key back to Telescope.

local M = {}

local function notes() return require("notes") end
local function sidebar() return require("notes.sidebar") end

-- Picks kept, newest first, so the palette lists them first.
M.RECENT_MAX = 8
local function recent_file() return vim.fn.stdpath("state") .. "/notes-palette-recent" end
local function recent()
  local ok, lines = pcall(vim.fn.readfile, recent_file())
  return ok and lines or {}
end
local function remember(name)
  local list = { name }
  for _, other in ipairs(recent()) do
    if other ~= name and #list < M.RECENT_MAX then table.insert(list, other) end
  end
  vim.fn.mkdir(vim.fs.dirname(recent_file()), "p")
  pcall(vim.fn.writefile, list, recent_file())
end

-- A list command acts on the row the cursor is on. Opened from a note, the
-- list is shown with its cursor on that note first.
local function on_note(fn)
  return function(ctx)
    local n = notes()
    if not sidebar().window() then n.layout() end
    local sb = sidebar()
    if not ctx.from_list then
      local path = n.current_path() and vim.fs.normalize(n.current_path())
      if not path then return end
      -- A note in Recently Deleted is listed only there.
      if require("notes.store").in_trash(path) and sb.state.folder ~= require("notes.store").TRASH then
        sb.show({ folder = require("notes.store").TRASH })
      end
      sb.reveal(path)
      -- Never another note than the one open.
      local item = sb.state.items[vim.api.nvim_win_get_cursor(sb.window())[1]]
      if not (item and item.note == path) then return vim.notify("notes: couldn't find the open note in the list") end
    end
    vim.api.nvim_set_current_win(sb.window())
    fn()
  end
end
-- One that acts on the list as a whole: shown, the cursor in it.
local function on_list(fn)
  return function()
    if not sidebar().window() then notes().layout() end
    vim.api.nvim_set_current_win(sidebar().window())
    fn()
  end
end

-- A Switchboard key, pressed in tmux as if typed: a root key, or one of
-- M-p's palette table, with the client the notes app is in.
local function press(key, palette_table)
  return function()
    if (vim.env.TMUX or "") == "" then return vim.notify("notes: that needs tmux") end
    local client = vim.trim(vim.fn.system(vim.env.TMUX_PANE and { "tmux", "display", "-p", "-t", vim.env.TMUX_PANE, "#{client_name}" } or { "tmux", "display", "-p", "#{client_name}" }))
    if palette_table then vim.fn.system({ "tmux", "switch-client", "-c", client, "-T", "palette" }) end
    vim.fn.system({ "tmux", "send-keys", "-K", "-c", client, key })
  end
end

-- The commands for where it was opened: { name, key, tag, run(ctx) }.
function M.commands(ctx)
  local sb = sidebar()
  local list = {
    { "Show or put away the list", "Ctrl-B", "view", function() notes().toggle_sidebar() end },
    { "Find a note", "Ctrl-F", "view", function() require("notes.finder").open() end },
    { "Search every note", "/", "view", on_list(sb.search) },
    { "Clear the search or tag", "x", "view", on_list(function() sb.show({ folder = sb.state.folder }) end) },
    { ctx.from_list and "Go to the note" or "Go to the list", "Tab", "view", function()
      if ctx.from_list then return notes().focus_note() end
      if not sb.window() then notes().layout() end
      vim.api.nvim_set_current_win(sb.window())
    end },
    { "Show the list's keys", "?", "view", on_list(sb.help) },
    { "Open what the list's cursor is on", "Enter", "view", on_list(function() sb.activate() end) },
    { "New note", "n", "note", on_list(sb.new_note) },
    { "New folder", "N", "note", on_list(sb.new_folder) },
    { "Rename the note", "R", "note", on_note(sb.rename) },
    { "Move the note to a folder", "m", "note", on_note(sb.move) },
    { "Pin or unpin the note", "p", "note", on_note(sb.pin) },
    { "Delete the note", "dd", "note", on_note(sb.delete) },
    { "Restore the note from Recently Deleted", "r", "note", on_note(sb.restore) },
    { "Mark the note to squash", "s", "note", on_note(sb.mark) },
    { "Squash the marked notes into one", "S", "note", on_list(sb.squash) },
    { "Open the note's tab", "t", "tabs", on_note(sb.tab) },
    { "Move Notes beside your work, or back to its own tab", "M-e", "tabs", press("M-e") },
    { "Choose a tab's note", "", "tabs", press("N", true) },
    { "Shape the note's checklist items as Linear tickets", "", "tabs", press("s", true) },
    { "Read a link from the note beside it", "", "links", press("o", true) },
    { "Search notes, Linear, GitHub and Slack", "M-S-f", "search", press("M-F") },
    { "Quit Notes", "q", "app", function() notes().quit() end },
  }
  if ctx.buf then
    local n = notes()
    local row = ctx.row
    table.insert(list, { "Tick, fold or follow what the cursor is on", "Enter", "note", function() n.act(ctx.buf, row, ctx.col, false) end })
    local format = require("notes.format")
    table.insert(list, { "Indent the item, or go to the next cell", "Tab", "note", function() format.tab(ctx.buf, 1) end })
    table.insert(list, { "Outdent the item, or go to the previous cell", "Shift-Tab", "note", function() format.tab(ctx.buf, -1) end })
    table.insert(list, { "Fold or unfold this heading", "Enter", "fold", function() require("notes.fold").toggle(ctx.buf, row) end })
    table.insert(list, { "Fold every heading", "zM", "fold", function() vim.cmd("normal! zM") end })
    table.insert(list, { "Unfold every heading", "zR", "fold", function() vim.cmd("normal! zR") end })
    local heading
    for _, item in ipairs(n.format_items(ctx.buf, row, row, false, ctx.typing_col)) do
      if #item == 1 then
        heading = item[1]
      else
        local action = item[3]
        table.insert(list, { heading .. ": " .. item[2], "Ctrl-P " .. item[1], "format", function() n.run_format(action, false) end })
      end
    end
  end
  return list
end

-- fzf's lines: the command's number, hidden, then its name, key and tag,
-- the recent ones first.
function M.lines(commands)
  local order = {}
  for at, name in ipairs(recent()) do order[name] = at end
  local numbered = {}
  for i, c in ipairs(commands) do table.insert(numbered, { i = i, c = c }) end
  table.sort(numbered, function(a, b)
    local ra, rb = order[a.c[1]] or math.huge, order[b.c[1]] or math.huge
    if ra ~= rb then return ra < rb end
    return a.i < b.i
  end)
  return vim.tbl_map(function(e)
    return ("%d\t%-52s %-10s \27[38;5;244m#%s\27[0m"):format(e.i, e.c[1], e.c[2], e.c[3])
  end, numbered)
end

function M.open()
  local origin = vim.api.nvim_get_current_win()
  local from_list = origin == sidebar().window()
  local buf = not from_list and vim.b[vim.api.nvim_win_get_buf(origin)].notes and vim.api.nvim_win_get_buf(origin) or nil
  local typing_col = vim.api.nvim_get_mode().mode == "i" and vim.api.nvim_win_get_cursor(origin)[2] or nil
  vim.cmd("stopinsert")
  local ctx = { from_list = from_list, buf = buf, row = vim.api.nvim_win_get_cursor(origin)[1], col = vim.api.nvim_win_get_cursor(origin)[2], typing_col = typing_col }
  -- The setting, as tmux holds it; on outside tmux.
  local off = (vim.env.TMUX or "") ~= "" and vim.trim(vim.fn.system({ "tmux", "show", "-gqv", "@setting_notes_palette" })) == "off"
  if off then
    local ok, builtin = pcall(require, "telescope.builtin")
    if ok then builtin.keymaps() end
    return
  end
  if vim.fn.executable("fzf") == 0 then
    vim.notify("notes: Ctrl-Shift-P needs fzf (brew install fzf)", vim.log.levels.WARN)
    return
  end
  local commands = M.commands(ctx)
  local list, chosen = vim.fn.tempname(), vim.fn.tempname()
  vim.fn.writefile(M.lines(commands), list)
  local term = vim.api.nvim_create_buf(false, true)
  local width = math.min(84, vim.o.columns - 4)
  local height = math.min(#commands + 2, math.floor(vim.o.lines * 0.7))
  local win = vim.api.nvim_open_win(term, true, {
    relative = "editor", width = width, height = height,
    row = math.floor((vim.o.lines - height) / 3), col = math.floor((vim.o.columns - width) / 2),
    style = "minimal", border = "rounded", title = " Notes ", title_pos = "center",
  })
  local fzf = "fzf --ansi --delimiter='\\t' --with-nth=2 --no-sort --layout=reverse --exact --prompt='command> '"
  local cmd = string.format("%s < %s > %s", fzf, vim.fn.shellescape(list), vim.fn.shellescape(chosen))
  local job = vim.fn.jobstart(cmd, { term = true, on_exit = function(_, code)
    vim.schedule(function()
      if vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
      if vim.api.nvim_buf_is_valid(term) then pcall(vim.api.nvim_buf_delete, term, { force = true }) end
      local picked = code == 0 and vim.fn.filereadable(chosen) == 1 and vim.fn.readfile(chosen)[1] or nil
      os.remove(list)
      os.remove(chosen)
      local command = picked and commands[tonumber(vim.split(picked, "\t")[1])]
      if vim.api.nvim_win_is_valid(origin) then vim.api.nvim_set_current_win(origin) end
      if not command then return end
      remember(command[1])
      command[4](ctx)
    end)
  end })
  require("notes.easy").close_button(win, function() vim.fn.jobstop(job) end)
  vim.cmd("startinsert")
end

return M
