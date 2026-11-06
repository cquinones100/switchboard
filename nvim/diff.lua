-- A Neovim for diffs alone, the one M-d opens in a pane's place
-- (tmux/pane-in-place.sh): `nvim -u ~/.config/nvim/diff.lua --noplugin …`.
-- Only diffview, the colorscheme and its icons, taken from where lazy.nvim
-- keeps them, set up as your normal config sets them up (their specs in
-- lua/plugins/); none of the language servers, completion, Copilot, gitsigns,
-- statusline or file tree the editor has, which have nothing to do in a diff
-- and only slow it and add noise.
--
-- --noplugin keeps everything else out, so diffview's own plugin/ file, which
-- defines its commands, is run by hand once it is on the runtimepath.

local lazy = vim.fn.stdpath("data") .. "/lazy"
local config = vim.fn.stdpath("config")
for _, name in ipairs({ "diffview.nvim", "mini.base16", "nvim-web-devicons" }) do
  if vim.uv.fs_stat(lazy .. "/" .. name) then vim.opt.runtimepath:prepend(lazy .. "/" .. name) end
end

-- Web links through tmux/open-url.sh, in their Chrome profile, as in init.lua.
require("open_url")

vim.o.termguicolors = true
vim.o.mouse = "a"
vim.o.number = false
vim.o.signcolumn = "no"
vim.o.laststatus = 0
-- diffview opens in a tab of its own; the empty one it came from would only add
-- a tabline naming it.
vim.o.showtabline = 0
vim.o.showmode = false
-- Nothing past the end of a buffer: the file list and a short file stop
-- there, with no column of ~ below them; and no lines drawn between windows,
-- the list and each side set apart by their own text.
vim.opt.fillchars:append({ eob = " ", vert = " ", horiz = " ", horizup = " ", horizdown = " ", vertleft = " ", vertright = " ", verthoriz = " " })
vim.o.swapfile = false
vim.o.shadafile = "NONE"
vim.o.clipboard = "unnamedplus"
-- Search as in the editor (init.lua): any case, until the search has a capital.
vim.o.ignorecase = true
vim.o.smartcase = true

-- Syntax from tree-sitter where Neovim has the language's parser.
vim.api.nvim_create_autocmd("FileType", {
  callback = function(args) pcall(vim.treesitter.start, args.buf) end,
})

-- The colorscheme and diffview, as lua/plugins/ configures them.
local function setup(file)
  local ok, spec = pcall(dofile, config .. "/lua/plugins/" .. file)
  if ok and spec.config then spec.config(spec, spec.opts or {}) end
end
setup("colorscheme.lua")
vim.cmd.runtime("plugin/diffview.lua")
setup("diffview.lua")

-- The window on that side, else tmux's pane there (the sidebar to the
-- left), as vim-tmux-navigator does in the editor, which this Neovim doesn't
-- load: C-w h, j, k and l, which tmux passes to Neovim (is_vim in tmux.conf),
-- and M-Left, M-Down, M-Up and M-Right (pane_arrows setting), the M-arrows
-- unmapped with the setting off.
local function move(dir)
  if vim.fn.winnr(dir) ~= vim.fn.winnr() then return vim.cmd.wincmd(dir) end
  if vim.env.TMUX then vim.fn.system({ "tmux", "select-pane", "-" .. ({ h = "L", j = "D", k = "U", l = "R" })[dir] }) end
end
for key, dir in pairs({ Left = "h", Down = "j", Up = "k", Right = "l" }) do
  vim.keymap.set("n", "<C-w>" .. dir, function() move(dir) end, { desc = "The window or pane " .. key:lower() })
  vim.keymap.set({ "n", "x", "s" }, "<M-" .. key .. ">", function()
    if vim.env.TMUX and vim.trim(vim.fn.system({ "tmux", "show", "-gqv", "@setting_pane_arrows" })) == "off" then
      return vim.api.nvim_feedkeys(vim.keycode("<M-" .. key .. ">"), "n", false)
    end
    move(dir)
  end, { desc = "The window or pane " .. key:lower() })
end

-- Easy mode (the easy_keys setting, off by default): a row of buttons at the
-- bottom for the keys the list takes, then the keys themselves named
-- (key_help_line setting). A button runs its key in the file list, so it
-- does what the key does.
local settings = { easy_keys = false, key_help_line = true, hover_highlight = true }
if vim.env.TMUX then
  for _, line in ipairs(vim.fn.systemlist({ "tmux", "show", "-g" })) do
    local key, value = line:match("^@setting_([%w_]+)%s+(%S+)")
    -- tmux quotes a value when it needs to: "on" as well as on.
    value = value and value:gsub('^"(.*)"$', "%1")
    if key and settings[key] ~= nil then settings[key] = value == "on" end
  end
end
-- Set with --cmd first, as the tests do, in place of the settings.
if vim.g.diff_easy ~= nil then settings.easy_keys = vim.g.diff_easy end
if vim.g.diff_key_help ~= nil then settings.key_help_line = vim.g.diff_key_help end

local BUTTONS = {
  { label = "◀ Chapter", keys = "[c" },
  { label = "Chapter ▶", keys = "]c" },
  { label = "◀ Page", keys = "[p" },
  { label = "Page ▶", keys = "]p" },
  { label = "Widen", keys = "E" },
  { label = "Copy link", keys = "yl" },
  { label = "Close", keys = "q" },
}
local HELP = "↑↓ choose a file · Enter open it · ]c [c chapters · ]p [p pages · E widen · yl copy a link · q close"
-- Ask, with the diff_ask setting on (ga, lua/plugins/diffview.lua).
if vim.g.diff_ask then
  table.insert(BUTTONS, #BUTTONS, { label = "Ask", keys = "ga" })
  HELP = HELP:gsub(" · q close$", " · ga ask Claude · q close")
end

local hover
-- The bar, the button under the mouse lit, and each button's { from, to }
-- screen columns, 1-based.
local function bar(hovered)
  local parts, spans, col = { " " }, {}, 2
  for i, button in ipairs(BUTTONS) do
    local text = " " .. button.label .. " "
    local width = vim.fn.strdisplaywidth(text)
    table.insert(parts, ("%%%d@v:lua.diff_button_click@%%#%s#%s%%X%%#StatusLine# "):format(i, i == hovered and "SwitchboardButtonHover" or "SwitchboardButton", text))
    spans[i] = { col, col + width - 1 }
    col = col + width + 1
  end
  -- %< cuts the help, never a button, when the screen is too narrow for both.
  if settings.key_help_line then table.insert(parts, "%<%#Comment# " .. HELP) end
  return table.concat(parts), spans
end

local function press(index)
  local button = BUTTONS[index]
  local view = require("diffview.lib").get_current_view()
  if not (button and view) then return end
  if view.panel and view.panel:is_open() then view.panel:focus() end
  vim.api.nvim_feedkeys(vim.keycode(button.keys), "m", false)
end
_G.diff_button_click = function(index, _, mouse)
  if mouse == "l" then press(index) end
end

if settings.easy_keys then
  vim.api.nvim_set_hl(0, "SwitchboardButton", { default = true, link = "TabLine" })
  vim.api.nvim_set_hl(0, "SwitchboardButtonHover", { default = true, link = "PmenuSel" })
  vim.o.laststatus = 3
  vim.o.statusline = (bar())
end
-- The button under the mouse lit (the hover_highlight setting).
if settings.easy_keys and settings.hover_highlight then
  vim.o.mousemoveevent = true
  vim.keymap.set({ "n", "x" }, "<MouseMove>", function()
    local pos = vim.fn.getmousepos()
    local now
    if pos.screenrow == vim.o.lines - vim.o.cmdheight then
      for i, span in ipairs(select(2, bar())) do
        if pos.screencol >= span[1] and pos.screencol <= span[2] then now = i end
      end
    end
    if now ~= hover then
      hover = now
      vim.o.statusline = (bar(hover))
    end
  end, { desc = "Light the button under the mouse" })
end
