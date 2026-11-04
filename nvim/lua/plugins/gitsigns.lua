-- The request audit (lib/requests.js): a GitHub lookup through gh, one line
-- in today's file, when gh is there to make it.
local function count_request(source)
  if vim.fn.executable("gh") == 0 then return end
  pcall(function()
    local dir = vim.fn.expand("~/.local/state/switchboard/requests")
    vim.fn.mkdir(dir, "p")
    local log = io.open(dir .. "/" .. os.date("%Y-%m-%d") .. ".tsv", "a")
    if log then
      log:write(("%d\tGitHub\tGraphQL\t%s\n"):format(os.time(), source))
      log:close()
    end
  end)
end

return {
  "lewis6991/gitsigns.nvim",
  event = { "BufReadPre", "BufNewFile" },
  opts = {
    current_line_blame = true,
    current_line_blame_opts = {
      delay = 300,
    },
  },
  keys = {
    {
      "<leader>gp",
      function()
        local file = vim.fn.expand("%")
        local line = vim.api.nvim_win_get_cursor(0)[1]
        local result = vim.fn.system({ "git", "blame", "-L", line .. "," .. line, "--porcelain", file })
        local sha = result:match("^(%x+)")
        if not sha or sha:match("^0+$") then
          vim.notify("No commit for this line (uncommitted change)", vim.log.levels.WARN)
          return
        end
        count_request("blame PR")
        local pr = vim.fn.system({ "gh", "pr", "list", "--search", sha, "--state", "merged", "--json", "number", "--jq", ".[0].number" })
        pr = vim.trim(pr)
        if pr == "" then
          vim.notify("No PR found for this commit, opening commit instead", vim.log.levels.INFO)
          vim.fn.jobstart({ "gh", "browse", sha }, { detach = true })
        else
          count_request("blame PR")
          vim.fn.jobstart({ "gh", "pr", "view", pr, "--web" }, { detach = true })
        end
      end,
      desc = "Open blame PR on GitHub",
    },
    {
      "<leader>go",
      function()
        local file = vim.fn.expand("%:.")
        local line = vim.api.nvim_win_get_cursor(0)[1]
        count_request("open the file on GitHub")
        local main = vim.trim(vim.fn.system({ "gh", "repo", "view", "--json", "defaultBranchRef", "--jq", ".defaultBranchRef.name" }))
        vim.fn.jobstart({ "gh", "browse", file .. ":" .. line, "--branch", main }, { detach = true })
      end,
      desc = "Open file on GitHub",
    },
    {
      "<leader>gb",
      "<cmd>Gitsigns toggle_current_line_blame<CR>",
      desc = "Toggle git blame",
    },
  },
}
