-- Neovim in Ghostty's own colors: the theme Ghostty is using now, read from
-- its theme file, laid out as a base16 scheme by mini.base16, so Neovim, the
-- notes app and M-d's diffs match the terminal and every tmux pane beside
-- them. The theme is the one tmux/theme.sh last wrote to
-- ~/.config/ghostty/theme-mode.conf (Tomorrow Night or GitHub Light High
-- Contrast as it stands), else the `theme` in Ghostty's own config, its
-- dark or light half by `light:…,dark:…` and Neovim's background. The
-- background is left transparent, as the terminal's, so tmux's dimming of
-- inactive panes (window-style) still shows. Without the theme file or
-- mini.base16, Neovim's own default colors.

local home = vim.fn.expand("~")

local function read(path)
  local file = io.open(path, "r")
  if not file then return nil end
  local text = file:read("*a")
  file:close()
  return text
end

local function theme_name()
  for _, path in ipairs({ home .. "/.config/ghostty/theme-mode.conf", home .. "/.config/ghostty/config" }) do
    local name = ("\n" .. (read(path) or "") .. "\n"):match("\n%s*theme%s*=%s*([^\n]-)%s*\n")
    if name and name ~= "" then
      if name:find(",") then
        local want = vim.o.background == "light" and "light" or "dark"
        name = name:match(want .. ":([^,]+)") or name:match(":([^,]+)")
      end
      return name and vim.trim(name)
    end
  end
end

local function theme_file(name)
  local dirs = {
    home .. "/.config/ghostty/themes",
    vim.env.GHOSTTY_RESOURCES_DIR and vim.env.GHOSTTY_RESOURCES_DIR .. "/themes",
    "/Applications/Ghostty.app/Contents/Resources/ghostty/themes",
  }
  for _, dir in pairs(dirs) do
    local text = dir and read(dir .. "/" .. name)
    if text then return text end
  end
end

local function hex(color) return tonumber(color:sub(2), 16) end
local function mix(a, b, amount)
  local out = 0
  for _, shift in ipairs({ 16, 8, 0 }) do
    local ca, cb = math.floor(hex(a) / 2 ^ shift) % 256, math.floor(hex(b) / 2 ^ shift) % 256
    out = out + math.floor(ca * (1 - amount) + cb * amount + 0.5) * 2 ^ shift
  end
  return ("#%06x"):format(out)
end
local function light(color)
  local n = hex(color)
  return (0.299 * (math.floor(n / 65536) % 256) + 0.587 * (math.floor(n / 256) % 256) + 0.114 * (n % 256)) > 140
end

local name = theme_name()
local text = name and theme_file(name)
local ok, base16 = pcall(require, "mini.base16")
if not (text and ok) then
  vim.cmd("highlight clear")
  vim.g.colors_name = "ghostty"
  return
end

local ansi, bg, fg, selection = {}, nil, nil, nil
for line in text:gmatch("[^\n]+") do
  local n, color = line:match("^%s*palette%s*=%s*(%d+)%s*=%s*(#%x%x%x%x%x%x)")
  if n then ansi[tonumber(n)] = color:lower() end
  bg = bg or line:match("^%s*background%s*=%s*(#%x%x%x%x%x%x)")
  fg = fg or line:match("^%s*foreground%s*=%s*(#%x%x%x%x%x%x)")
  selection = selection or line:match("^%s*selection%-background%s*=%s*(#%x%x%x%x%x%x)")
end
bg, fg = (bg or ansi[0] or "#000000"):lower(), (fg or ansi[7] or "#ffffff"):lower()
for i = 0, 15 do ansi[i] = ansi[i] or (i % 8 == 0 and bg or fg) end

vim.o.background = light(bg) and "light" or "dark"
-- The terminal's colors as base16 has them: the background and its shades
-- toward the text, the text, then the ANSI hues, with the two base16 has and
-- ANSI doesn't (orange, brown) mixed from them.
base16.setup({
  palette = {
    base00 = bg,
    base01 = mix(bg, fg, 0.07),
    base02 = (selection or mix(bg, fg, 0.15)):lower(),
    base03 = mix(bg, fg, 0.5),
    base04 = mix(bg, fg, 0.7),
    base05 = fg,
    base06 = fg,
    base07 = fg,
    base08 = ansi[1],
    base09 = mix(ansi[1], ansi[3], 0.5),
    base0A = ansi[3],
    base0B = ansi[2],
    base0C = ansi[6],
    base0D = ansi[4],
    base0E = ansi[5],
    base0F = mix(ansi[1], bg, 0.4),
  },
  use_cterm = false,
})
for _, group in ipairs({ "Normal", "NormalNC", "SignColumn", "LineNr", "FoldColumn", "EndOfBuffer", "CursorLineNr" }) do
  local hl = vim.api.nvim_get_hl(0, { name = group, link = false })
  hl.bg = nil
  vim.api.nvim_set_hl(0, group, hl)
end
-- Switchboard's buttons (the notes app's toolbar, M-d's easy mode): a padded
-- label on a shade of the background, lit in the accent blue under the mouse,
-- as tmux's Commands button (@button_style) is drawn.
vim.api.nvim_set_hl(0, "SwitchboardButton", { bg = mix(bg, fg, 0.18), fg = fg })
vim.api.nvim_set_hl(0, "SwitchboardButtonHover", { bg = ansi[4], fg = bg, bold = true })
-- The terminal's background, for anything that blends toward it (diffview's
-- line colors), since Normal has none.
vim.g.ghostty_background = bg
vim.g.colors_name = "ghostty"
