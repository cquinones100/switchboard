-- ── Shared Settings ──────────────────────────────────
vim.g.mapleader = " "
vim.g.maplocalleader = " "

vim.opt.number = true
vim.opt.relativenumber = true
vim.opt.scrolloff = 8
vim.opt.tabstop = 2
vim.opt.softtabstop = 2
vim.opt.shiftwidth = 2
vim.opt.expandtab = true
vim.opt.clipboard = "unnamed"
vim.opt.hlsearch = false
vim.opt.incsearch = true
vim.opt.splitright = true
vim.opt.splitbelow = true
vim.opt.updatetime = 50

-- Web links through tmux/open-url.sh, in their Chrome profile.
require("open_url")

-- Yank entire line with Shift+Y
vim.keymap.set("n", "<S-y>", "yy")

-- Save with Ctrl+S
vim.keymap.set("n", "<C-s>", vim.cmd.w)

-- Move highlighted text up or down
vim.keymap.set("v", "J", ":m '>+1<CR>gv=gv")
vim.keymap.set("v", "K", ":m '<-2<CR>gv=gv")

-- Keep cursor in the middle when jumping, go to boundary when at end
vim.keymap.set("n", "<C-d>", function()
  if vim.fn.line("w$") >= vim.fn.line("$") then
    vim.api.nvim_win_set_cursor(0, { vim.fn.line("$"), 0 })
  else
    local keys = vim.api.nvim_replace_termcodes("<C-d>M", true, false, true)
    vim.api.nvim_feedkeys(keys, "n", false)
  end
end)
vim.keymap.set("n", "<C-u>", function()
  if vim.fn.line("w0") <= 1 then
    vim.api.nvim_win_set_cursor(0, { 1, 0 })
  else
    local keys = vim.api.nvim_replace_termcodes("<C-u>M", true, false, true)
    vim.api.nvim_feedkeys(keys, "n", false)
  end
end)

-- Replace currently hovered word
vim.keymap.set("n", "<leader>s", [[:%s/\<<C-r><C-w>\>/<C-r><C-w>/gI<Left><Left><Left>]])

-- Copy relative path to clipboard
vim.keymap.set("n", "<leader>cp", function() vim.fn.setreg("+", vim.fn.expand("%:.")) end, { desc = "Copy relative file path" })
vim.keymap.set("n", "<leader>cP", function() vim.fn.setreg("+", vim.fn.expand("%:p")) end, { desc = "Copy absolute file path" })

-- Make Ctrl+C behave exactly like Escape (also fires InsertLeave)
vim.keymap.set({ "i", "n", "v", "x", "s" }, "<C-c>", "<Esc>")

-- ── VSCode-specific ──────────────────────────────────
if vim.g.vscode then
  vim.keymap.set("n", "<S-c-c>", function() vim.fn.setreg("+", vim.fn.expand("%:p")) end)
  vim.keymap.set("n", "<C-a>", "ggVG")
  return
end

-- ── Standalone Settings ──────────────────────────────
vim.opt.termguicolors = true
vim.opt.signcolumn = "yes"
vim.opt.cursorline = true
vim.opt.wrap = false
vim.opt.linebreak = true
vim.opt.breakindent = true
vim.opt.showbreak = "↪ "
vim.api.nvim_create_autocmd("FileType", {
  pattern = { "markdown", "text", "gitcommit" },
  callback = function() vim.opt_local.wrap = true end,
})
-- Move by screen line on wrapped text, but keep counts (10j) on real lines
vim.keymap.set({ "n", "x" }, "j", "v:count == 0 ? 'gj' : 'j'", { expr = true })
vim.keymap.set({ "n", "x" }, "k", "v:count == 0 ? 'gk' : 'k'", { expr = true })
vim.opt.undofile = true
vim.opt.autoread = true
vim.api.nvim_create_autocmd({ "FocusGained", "BufEnter", "CursorHold" }, {
  command = "checktime",
})
vim.opt.ignorecase = true
vim.opt.smartcase = true

vim.filetype.add({
  extension = {
    tfvars = "terraform-vars",
  },
})

-- ── Bootstrap lazy.nvim ──────────────────────────────
-- A new machine gets the lazy.nvim commit lazy-lock.json records, not
-- whatever its stable branch holds that day; the plugins follow the lockfile
-- with :Lazy restore.
local lazypath = vim.fn.stdpath("data") .. "/lazy/lazy.nvim"
local lazy_commit = "306a05526ada86a7b30af95c5cc81ffba93fef97"
if not vim.uv.fs_stat(lazypath) then
  vim.fn.system({ "git", "clone", "--filter=blob:none", "https://github.com/folke/lazy.nvim.git", lazypath })
  vim.fn.system({ "git", "-C", lazypath, "checkout", "--quiet", lazy_commit })
end
vim.opt.rtp:prepend(lazypath)

require("lazy").setup("plugins")

-- ── LSP Hover ────────────────────────────────────────
vim.o.winborder = "rounded"

-- ── LSP Keymaps ──────────────────────────────────────
vim.api.nvim_create_autocmd("LspAttach", {
  callback = function(event)
    local map = function(keys, func)
      vim.keymap.set("n", keys, func, { buffer = event.buf })
    end
    map("gd", vim.lsp.buf.definition)
    map("<C-]>", vim.lsp.buf.definition)
    map("gr", vim.lsp.buf.references)
    map("gi", vim.lsp.buf.implementation)
    map("K", vim.lsp.buf.hover)
    map("gh", function()
      local diagnostics = vim.diagnostic.get(0, { lnum = vim.api.nvim_win_get_cursor(0)[1] - 1 })
      if #diagnostics > 0 then
        vim.diagnostic.open_float()
      else
        vim.lsp.buf.hover()
      end
    end)
    map("<leader>rn", vim.lsp.buf.rename)
    map("<leader>ca", vim.lsp.buf.code_action)
    map("<C-S-k>", vim.diagnostic.goto_prev)
    map("<C-S-j>", vim.diagnostic.goto_next)
  end,
})

-- The user layer: your own Neovim settings, in ~/.config/switchboard/nvim.lua,
-- run last so they win, and never in this repository, so updates leave them be.
local user_layer = vim.fn.expand("~/.config/switchboard/nvim.lua")
if vim.uv.fs_stat(user_layer) then
  dofile(user_layer)
end
