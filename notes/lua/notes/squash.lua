-- Squashing notes: two or more marked in the sidebar (s marks, S squashes)
-- become one, merged by a headless Claude, after you say yes, since it sends
-- their text to Claude. Sonnet merges: careful enough not to drop an item,
-- which Haiku would risk, at a fraction of Opus's cost (the squash_model
-- option). Where two notes clash and nothing says which is right, the one
-- changed last wins. The new note goes in the first one's folder, the originals go to
-- Recently Deleted, so nothing is lost, and every tmux tab linked to one of
-- them (tmux/notes-open.sh's @tab_note and @tab_note_last) links to the new
-- one.

local M = {}

local store = require("notes.store")

M.options = { model = "sonnet" }

local PROMPT = table.concat({
  "Merge the notes below into one Markdown note.",
  "Its first line is '# ' and a title that covers them all.",
  "Keep every item, every checklist box as it is ([ ] or [x]), every link and every detail, and group what belongs together under headings;",
  "drop only what is said twice, keeping the fuller one. Leave the ticked items of a checklist under its unticked ones, a blank line between.",
  "Each note says when it was last changed. Where two of them disagree (an item ticked in one and not the other, a different date, number or decision) and nothing in them says which is right, keep the version from the note changed most recently.",
  "Write nothing but the note: no preamble, no code fence around it.",
}, " ")

-- The tabs linked to one of `from` (paths in the notes directory) link to `to`.
local function relink(from, to)
  if (vim.env.TMUX or "") == "" then return end
  local listed = vim.system({ "tmux", "list-windows", "-a", "-F", "#{window_id}\t#{@tab_note}\t#{@tab_note_last}" }, { text = true }):wait()
  if listed.code ~= 0 then return end
  local old = {}
  for _, rel in ipairs(from) do old[rel] = true end
  for line in (listed.stdout or ""):gmatch("[^\n]+") do
    local window, note, last = line:match("^([^\t]*)\t([^\t]*)\t(.*)$")
    if window and old[note] then vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_note", to }):wait() end
    if window and old[last] then vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_note_last", to }):wait() end
  end
end

-- Squashes `paths` (absolute, two or more), calling done(new path) or
-- done(nil, why) when it is over. The merge runs in the background.
function M.run(paths, done)
  local parts = {}
  for i, path in ipairs(paths) do
    local f = io.open(path, "r")
    if not f then return done(nil, "couldn't read " .. store.relative(path)) end
    -- When it was last changed, for the prompt's rule on clashes, in UTC so
    -- no clock change puts two out of order.
    local stat = vim.uv.fs_stat(path)
    local changed = stat and os.date("!%Y-%m-%dT%H:%M:%SZ", stat.mtime.sec) or "unknown"
    table.insert(parts, ("<note %d changed=\"%s\">\n%s\n</note %d>"):format(i, changed, f:read("*a"), i))
    f:close()
  end
  local ok, job = pcall(vim.system, { "claude", "-p", "--model", M.options.model, "--tools", "", "--no-session-persistence", PROMPT }, { text = true, stdin = table.concat(parts, "\n\n") }, function(out)
    vim.schedule(function()
      local text = vim.trim(out.stdout or ""):gsub("^```%w*\n", ""):gsub("\n```$", "")
      if out.code ~= 0 or not text:match("^# %S") then
        return done(nil, "Claude didn't merge them" .. (out.stderr ~= "" and (": " .. vim.trim(out.stderr):match("[^\n]*")) or ""))
      end
      local folder = vim.fs.dirname(store.relative(paths[1]))
      local path = store.create(folder == "." and "" or folder, store.title_of(text))
      local f = io.open(path, "w")
      if not f then return done(nil, "couldn't write the new note") end
      f:write(text .. "\n")
      f:close()
      local rels = {}
      for _, old in ipairs(paths) do
        table.insert(rels, store.relative(old))
        require("notes").forget(old)
        store.delete(old)
      end
      relink(rels, store.relative(path))
      done(path)
    end)
  end)
  if not ok then done(nil, "Claude Code isn't installed") end
  return job
end

-- A box over the notes while a squash runs: a spinner, the notes being merged,
-- the seconds so far, and Esc to cancel, since Claude takes a while. Returns a
-- function that closes it. `cancel` is called on Esc.
function M.progress(titles, model, cancel)
  local frames = { "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏" }
  local started = vim.uv.now()
  local buf = vim.api.nvim_create_buf(false, true)
  local lines = function(frame)
    local out = { "", ("  %s Squashing %d notes with Claude (%s)"):format(frames[frame], #titles, model), "" }
    for _, title in ipairs(titles) do table.insert(out, "    · " .. title) end
    table.insert(out, "")
    table.insert(out, ("  %ds so far · it usually takes up to half a minute · Esc cancels"):format(math.floor((vim.uv.now() - started) / 1000)))
    table.insert(out, "")
    return out
  end
  local width = 4
  for _, line in ipairs(lines(1)) do width = math.max(width, vim.fn.strdisplaywidth(line) + 2) end
  width = math.min(width, vim.o.columns - 4)
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines(1))
  local win = vim.api.nvim_open_win(buf, true, {
    relative = "editor", style = "minimal", border = "rounded", title = " Squash ", title_pos = "center",
    width = width, height = #titles + 6, row = math.max(1, math.floor((vim.o.lines - #titles - 6) / 2) - 1), col = math.floor((vim.o.columns - width) / 2),
  })
  local timer = vim.uv.new_timer()
  local frame = 1
  timer:start(100, 100, vim.schedule_wrap(function()
    if not vim.api.nvim_win_is_valid(win) then return end
    frame = frame % #frames + 1
    vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines(frame))
  end))
  local closed = false
  local close = function()
    if closed then return end
    closed = true
    timer:stop()
    timer:close()
    if vim.api.nvim_win_is_valid(win) then vim.api.nvim_win_close(win, true) end
  end
  vim.keymap.set("n", "<Esc>", function()
    close()
    cancel()
  end, { buffer = buf, nowait = true })
  return close
end

return M
