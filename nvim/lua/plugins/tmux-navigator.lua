-- An M-arrow: vim-tmux-navigator's move, unless tmux's pane_arrows setting
-- is off, then the key unmapped.
local function arrow(key, dir)
  return function()
    local off = vim.env.TMUX and vim.trim(vim.fn.system({ "tmux", "show", "-gqv", "@setting_pane_arrows" })) == "off"
    if off then return vim.api.nvim_feedkeys(vim.keycode(key), "n", false) end
    vim.cmd("TmuxNavigate" .. dir)
  end
end

return {
  "christoomey/vim-tmux-navigator",
  cmd = {
    "TmuxNavigateLeft",
    "TmuxNavigateDown",
    "TmuxNavigateUp",
    "TmuxNavigateRight",
  },
  init = function()
    vim.g.tmux_navigator_no_mappings = 1
  end,
  keys = {
    { "<C-w>h", "<cmd>TmuxNavigateLeft<cr>",  mode = "n" },
    { "<C-w>j", "<cmd>TmuxNavigateDown<cr>",  mode = "n" },
    { "<C-w>k", "<cmd>TmuxNavigateUp<cr>",    mode = "n" },
    { "<C-w>l", "<cmd>TmuxNavigateRight<cr>", mode = "n" },
    -- M-arrows, which tmux passes to Neovim (tmux.conf): with the
    -- pane_arrows setting off, the key as Neovim has it.
    { "<M-Left>", arrow("<M-Left>", "Left"), mode = { "n", "i", "x", "s" } },
    { "<M-Down>", arrow("<M-Down>", "Down"), mode = { "n", "i", "x", "s" } },
    { "<M-Up>", arrow("<M-Up>", "Up"), mode = { "n", "i", "x", "s" } },
    { "<M-Right>", arrow("<M-Right>", "Right"), mode = { "n", "i", "x", "s" } },
  },
}
