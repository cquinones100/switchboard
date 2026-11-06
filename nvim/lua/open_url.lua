-- A web link (gx, a link in the notes app or M-d's diff) opens through
-- tmux/open-url.sh, in the Chrome profile ~/.config/switchboard/browser.json
-- gives its site, as every link Switchboard opens does; anything else, and a
-- link when the script isn't linked, as Neovim opens it. gh's --web from
-- Neovim (gitsigns' GitHub keys) too, outside tmux, which sets GH_BROWSER
-- itself. Loaded by init.lua and diff.lua.
local open_url = vim.fn.expand("~/.config/tmux/open-url.sh")
if vim.fn.executable(open_url) == 0 then return end
local ui_open = vim.ui.open
vim.ui.open = function(path, opt)
  if type(path) == "string" and path:match("^https?://") then
    return vim.system({ open_url, path }, { detach = true }), nil
  end
  return ui_open(path, opt)
end
if not vim.env.GH_BROWSER then vim.env.GH_BROWSER = open_url end
