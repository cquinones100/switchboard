-- Ctrl-F: find a note with fzf, in a floating window. Every note but those in
-- Recently Deleted is listed as the sidebar lists them (pinned first, then
-- newest), each as its title, its folder and how it starts, any of which you
-- can type to match, with the note itself shown beside the list. Enter opens
-- it; Escape closes the finder.

local M = {}

local store = require("notes.store")

local function fit(text, width)
  text = text:gsub("[\t\n]", " ")
  if vim.fn.strdisplaywidth(text) <= width then return text .. string.rep(" ", width - vim.fn.strdisplaywidth(text)) end
  return vim.fn.strcharpart(text, 0, width - 1) .. "…"
end

-- fzf's lines: the note's path, hidden, then what is shown and matched.
function M.lines()
  return vim.tbl_map(function(note)
    local where = note.folder == "" and "Notes" or note.folder
    return table.concat({ note.rel, fit((note.pinned and "● " or "  ") .. note.title, 36) .. "  " .. fit(where, 18) .. "  " .. note.preview }, "\t")
  end, store.notes())
end

function M.open()
  if vim.fn.executable("fzf") == 0 then
    vim.notify("notes: Ctrl-F needs fzf (brew install fzf)", vim.log.levels.WARN)
    return
  end
  local list = vim.fn.tempname()
  local chosen = vim.fn.tempname()
  vim.fn.writefile(M.lines(), list)
  local buf = vim.api.nvim_create_buf(false, true)
  local width = math.floor(vim.o.columns * 0.85)
  local height = math.floor(vim.o.lines * 0.8)
  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor", width = width, height = height,
    row = math.floor((vim.o.lines - height) / 2), col = math.floor((vim.o.columns - width) / 2),
    style = "minimal", border = "rounded", title = " Find a note ", title_pos = "center",
  })
  local fzf = table.concat({
    "fzf",
    "--delimiter='\\t'",
    "--with-nth=2",
    "--layout=reverse",
    "--tiebreak=index",
    "--prompt='Find a note: '",
    "--preview='cat {1}'",
    "--preview-window='right,45%,wrap'",
  }, " ")
  local cmd = string.format("%s < %s > %s", fzf, vim.fn.shellescape(list), vim.fn.shellescape(chosen))
  local on_exit = function(_, code)
    vim.schedule(function()
      if vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
      if vim.api.nvim_buf_is_valid(buf) then pcall(vim.api.nvim_buf_delete, buf, { force = true }) end
      local picked = code == 0 and vim.fn.filereadable(chosen) == 1 and vim.fn.readfile(chosen)[1] or nil
      os.remove(list)
      os.remove(chosen)
      if picked and picked ~= "" then require("notes").open(store.absolute(vim.split(picked, "\t")[1])) end
    end)
  end
  local job = vim.fn.jobstart(cmd, { term = true, cwd = store.root(), on_exit = on_exit })
  require("notes.easy").close_button(win, function() vim.fn.jobstop(job) end)
  vim.cmd("startinsert")
end

return M
