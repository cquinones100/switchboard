-- Notes for people who don't know Vim's keys: the toolbar over a note (a
-- button for each of the Format menu's styles, notes_toolbar setting, on by
-- default), and easy mode (easy_keys setting, off by default): Shift-arrows
-- and the mouse select, typing replaces a selection, Cmd-S saves, Cmd-Z
-- undoes, Cmd-Shift-Z redoes and Cmd-X cuts, with a line naming the keys at
-- the bottom (key_help_line setting). Vim's keys keep working beside them.
--
-- Ghostty sends the Cmd keys as Ctrl+Alt+Shift ones (ghostty/config), which
-- tmux passes only to this app (tmux.conf); tmux reads a Cmd key as Alt.

local M = {}

-- The settings this module reads, from tmux's copies of them (tmux/settings.sh)
-- in one `tmux show`, their defaults outside tmux.
function M.settings()
  local on = { notes_toolbar = true, easy_keys = false, key_help_line = true }
  if (vim.env.TMUX or "") == "" then return on end
  for _, line in ipairs(vim.fn.systemlist({ "tmux", "show", "-g" })) do
    local key, value = line:match("^@setting_([%w_]+)%s+(%S+)")
    -- tmux quotes a value when it needs to: "on" as well as on.
    value = value and value:gsub('^"(.*)"$', "%1")
    if key and on[key] ~= nil then on[key] = value == "on" end
  end
  return on
end

-- ── The toolbar ─────────────────────────────────────────────────────────────

-- Each button names the Format menu item it runs (init.format_items), so the
-- two can't drift apart; ⋯ opens the whole menu.
M.BUTTONS = {
  { item = "Bold", label = "B" },
  { item = "Italic", label = "I" },
  { item = "Strikethrough", label = "S" },
  { item = "Monospaced text", label = "</>" },
  { item = "Title", label = "H1" },
  { item = "Heading", label = "H2" },
  { item = "Bulleted List", label = "•" },
  { item = "Checklist", label = "☐" },
  { item = "Table", label = "Table" },
  { item = "Link…", label = "Link" },
  { item = "Paste image", label = "Image" },
  { menu = true, label = "⋯" },
}

local hover
local toolbar_win

-- The winbar for the toolbar in a window `width` columns wide, the button
-- under the mouse lit, and where each button shown sits: { from, to } window
-- columns, 1-based, by its index in BUTTONS. The buttons that don't fit drop
-- off the end, ⋯ (the whole Format menu, which has them all) always kept.
function M.toolbar(hovered, width)
  local function text(button) return " " .. button.label .. " " end
  local menu = #M.BUTTONS
  local room = (width or math.huge) - 1 - (vim.fn.strdisplaywidth(text(M.BUTTONS[menu])) + 1)
  local shown, used = {}, 0
  for i = 1, menu - 1 do
    local w = vim.fn.strdisplaywidth(text(M.BUTTONS[i])) + 1
    if used + w > room then break end
    used = used + w
    table.insert(shown, i)
  end
  table.insert(shown, menu)
  local parts, spans, col = { " " }, {}, 2
  for _, i in ipairs(shown) do
    local label = text(M.BUTTONS[i])
    local w = vim.fn.strdisplaywidth(label)
    local group = i == hovered and "SwitchboardButtonHover" or "SwitchboardButton"
    table.insert(parts, ("%%%d@v:lua.notes_toolbar_click@%%#%s#%s%%X%%#WinBar# "):format(i, group, label))
    spans[i] = { col, col + w - 1 }
    col = col + w + 1
  end
  return table.concat(parts), spans
end

local function toolbar_width()
  return toolbar_win and vim.api.nvim_win_is_valid(toolbar_win) and vim.api.nvim_win_get_width(toolbar_win) or nil
end

local function draw_toolbar()
  if toolbar_width() then vim.wo[toolbar_win].winbar = (M.toolbar(hover, toolbar_width())) end
end

-- Runs a button on the note: on the selection when there is one, else on the
-- line or the word the cursor is on, as the Format menu does.
function M.click(index)
  local notes = require("notes")
  local button = M.BUTTONS[index]
  local win = notes.note_win
  if not (button and win and vim.api.nvim_win_is_valid(win)) then return end
  local buf = vim.api.nvim_win_get_buf(win)
  if not vim.b[buf].notes then return end
  local mode = vim.api.nvim_get_mode().mode
  local here = vim.api.nvim_get_current_win() == win
  local visual = here and mode:match("^[vVsS\22\19]") ~= nil
  local typing_col
  if not here then
    vim.api.nvim_set_current_win(win)
  elseif visual then
    -- Out of the selection, which leaves its marks for the style to read.
    vim.cmd("normal! \27")
  elseif mode == "i" then
    typing_col = vim.api.nvim_win_get_cursor(0)[2]
    vim.cmd("stopinsert")
  end
  local first, last = vim.fn.line("."), vim.fn.line(".")
  if visual then first, last = vim.fn.line("'<"), vim.fn.line("'>") end
  vim.schedule(function()
    if button.menu then return notes.menu(buf, visual, typing_col) end
    for _, item in ipairs(notes.format_items(buf, first, last, visual, typing_col)) do
      if item[2] == button.item then return notes.run_format(item[3], visual) end
    end
  end)
end

_G.notes_toolbar_click = function(index, _, button)
  if button == "l" then M.click(index) end
end

-- The button under the mouse is lit, from Neovim's mouse-move events.
local function track_hover()
  vim.o.mousemoveevent = true
  vim.keymap.set({ "n", "i", "x", "s" }, "<MouseMove>", function()
    if not (toolbar_win and vim.api.nvim_win_is_valid(toolbar_win)) then return end
    local pos = vim.fn.getmousepos()
    local top, left = unpack(vim.api.nvim_win_get_position(toolbar_win))
    local now
    if pos.screenrow == top + 1 then
      local _, spans = M.toolbar(nil, toolbar_width())
      local col = pos.screencol - left
      for i, span in pairs(spans) do
        if col >= span[1] and col <= span[2] then now = i end
      end
    end
    if now ~= hover then
      hover = now
      draw_toolbar()
    end
  end, { desc = "Light the toolbar button under the mouse" })
end

-- ── Easy mode ───────────────────────────────────────────────────────────────

local KEY_HELP = {
  n = "i type · Shift+arrows select · Ctrl-P format · Enter tick, fold or follow · Ctrl-F find · Ctrl-B list · Cmd-Z undo · Cmd-S save",
  i = "Esc stop typing · Shift+arrows select · Ctrl-P format · Tab indent · Enter next item · Cmd-Z undo",
  v = "Type to replace · Ctrl-P format the selection · Cmd-X cut · Ctrl-C copy · Esc cancel",
}

function M.key_help()
  local mode = vim.api.nvim_get_mode().mode
  local kind = mode:match("^[vVsS\22\19]") and "v" or mode:match("^i") and "i" or "n"
  return " " .. KEY_HELP[kind]
end
_G.notes_key_help = M.key_help

-- Takes the Select half off each map in maps that starts with a key that
-- types, buffer-local ones from buf; its Visual half stays.
local function typing_replaces(maps, buf)
  for _, m in ipairs(maps) do
    if not m.lhs:match("^<") then pcall(vim.keymap.del, "s", m.lhs, buf and { buffer = buf } or nil) end
  end
end

-- The Cmd keys, on a note's buffer.
function M.keys(buf)
  local map = function(modes, lhs, rhs, desc) vim.keymap.set(modes, lhs, rhs, { buffer = buf, silent = true, desc = desc }) end
  map({ "n", "i", "x", "s" }, "<C-M-S-s>", function() require("notes").save(buf) end, "Save (Cmd-S)")
  map("n", "<C-M-S-z>", "u", "Undo (Cmd-Z)")
  map("i", "<C-M-S-z>", "<C-o>u", "Undo (Cmd-Z)")
  map("n", "<C-M-S-y>", "<C-r>", "Redo (Cmd-Shift-Z)")
  map("i", "<C-M-S-y>", "<C-o><C-r>", "Redo (Cmd-Shift-Z)")
  -- With a selection too, which they end: Select mode's own maps, since a
  -- Visual mode map run there gives the selection back afterward.
  map("x", "<C-M-S-z>", "<Esc>u", "Undo (Cmd-Z)")
  map("x", "<C-M-S-y>", "<Esc><C-r>", "Redo (Cmd-Shift-Z)")
  map("s", "<C-M-S-z>", "<Esc>u", "Undo (Cmd-Z)")
  map("s", "<C-M-S-y>", "<Esc><C-r>", "Redo (Cmd-Shift-Z)")
  -- A Visual mode map, which Select mode runs as Visual too.
  map("v", "<C-M-S-x>", '"+d', "Cut (Cmd-X)")
  -- Copy, which Cmd-C can't be (it is Ghostty's own copy); y would type over
  -- a selection made in Select mode.
  map("v", "<C-c>", '"+y', "Copy the selection")
end

-- Once, as the app starts: what the settings turn on.
function M.start()
  M.on = M.settings()
  vim.api.nvim_set_hl(0, "SwitchboardButton", { default = true, link = "TabLine" })
  vim.api.nvim_set_hl(0, "SwitchboardButtonHover", { default = true, link = "PmenuSel" })
  if M.on.notes_toolbar then
    track_hover()
    -- Fitted again to the note window's width as it changes (M-e moving
    -- Notes beside a pane, the sidebar shown or put away).
    vim.api.nvim_create_autocmd({ "WinResized", "VimResized" }, { callback = draw_toolbar })
  end
  if M.on.easy_keys then
    -- Shifted arrows (and Home, End) start a selection and the plain ones end
    -- it; a selection made with them or the mouse is replaced by what is
    -- typed, as in any editor.
    vim.o.keymodel = "startsel,stopsel"
    -- tmux passes the Cmd keys only to a pane marked as running Notes this
    -- way, however Notes was started (M-N, or notes typed in a shell), and
    -- the mark goes when Notes does.
    if (vim.env.TMUX or "") ~= "" and (vim.env.TMUX_PANE or "") ~= "" then
      vim.fn.system({ "tmux", "set", "-p", "-t", vim.env.TMUX_PANE, "@notes_cmd_keys", "1" })
      vim.api.nvim_create_autocmd("VimLeavePre", {
        callback = function() vim.fn.system({ "tmux", "set", "-pu", "-t", vim.env.TMUX_PANE, "@notes_cmd_keys" }) end,
      })
    end
    vim.o.selectmode = "mouse,key"
    -- What is typed over a selection replaces it: a Visual mode map that
    -- starts with a key that types (the config's J and K, which move lines,
    -- or <leader>f with Space as the leader) would act in Select mode too, so
    -- its Select half goes, its Visual half kept. Maps on special keys
    -- (<Tab>, <C-…>) stay.
    typing_replaces(vim.api.nvim_get_keymap("s"))
    if M.on.key_help_line then
      pcall(function() require("lualine").hide({ place = { "statusline" }, unhide = false }) end)
      vim.o.laststatus = 3
      vim.o.statusline = "%#Comment#%{v:lua.notes_key_help()}"
      vim.api.nvim_create_autocmd("ModeChanged", { callback = function() vim.cmd("redrawstatus") end })
    end
  end
end

-- A note's window, as it is set up for a note.
function M.window(win)
  if not (M.on and M.on.notes_toolbar) then return end
  toolbar_win = win
  draw_toolbar()
end

-- A note's buffer, as it is set up.
-- A note's buffer, as it is set up, its type set (and so any FileType maps
-- of the config's made).
function M.buffer(buf)
  if not (M.on and M.on.easy_keys) then return end
  M.keys(buf)
  -- As for the config's global maps (M.start), its maps for this buffer.
  typing_replaces(vim.api.nvim_buf_get_keymap(buf, "s"), buf)
end

return M
