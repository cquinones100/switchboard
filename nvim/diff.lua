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
-- there, with no column of ~ below them.
vim.opt.fillchars:append({ eob = " " })
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
