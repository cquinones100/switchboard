-- Keeps an open note and its file in step when something outside the app
-- changes the file, such as a script or Claude marking a checklist item.
--
-- Each note's buffer remembers its base: the file as it was last read or
-- written. A file that no longer matches its base was changed from outside.
-- A buffer with nothing unsaved takes the change in place; one with unsaved
-- typing gets both merged, line by line (see merge). Every save checks
-- first (see check), so nothing is written over a change the app hasn't
-- seen, and a watch on the notes directory checks as soon as a file changes.

local M = {}

local uv = vim.uv

-- The base of each note buffer, by buffer number.
local base = {}

local function read(path)
  local file = io.open(path, "r")
  if not file then return nil end
  local text = file:read("*a")
  file:close()
  return text
end

local function lines_of(text)
  local lines = vim.split(text, "\n", { plain = true })
  if lines[#lines] == "" then table.remove(lines) end
  return lines
end

local function text_of(buf)
  return table.concat(vim.api.nvim_buf_get_lines(buf, 0, -1, false), "\n") .. "\n"
end

-- What changed from `from` to `to`, as base ranges: lines [lo, hi) of `from`
-- (0-based; lo == hi is an insertion before line lo) become `lines`.
local function hunks(from, to, side)
  local after = lines_of(to)
  return vim.tbl_map(function(h)
    local start_a, count_a, start_b, count_b = unpack(h)
    local lo = count_a > 0 and start_a - 1 or start_a
    return { lo = lo, hi = lo + count_a, lines = { unpack(after, start_b, start_b + count_b - 1) }, side = side }
  end, vim.diff(from, to, { result_type = "indices" }))
end

-- Merges two edits of `original`: changes to different lines both apply,
-- even on lines next to each other, as when you type in one checklist item
-- while something outside marks the next. Where both changed the same lines
-- differently, both versions are kept, yours first, rather than either being
-- lost. Returns the text and how many such places there were.
function M.merge(original, ours, theirs)
  local from = lines_of(original)
  local all = vim.list_extend(hunks(original, ours, "ours"), hunks(original, theirs, "theirs"))
  table.sort(all, function(a, b)
    if a.lo ~= b.lo then return a.lo < b.lo end
    if a.hi ~= b.hi then return a.hi < b.hi end
    return a.side < b.side
  end)
  -- Hunks that touch the same base lines, or insert at the same place.
  local groups = {}
  for _, h in ipairs(all) do
    local g = groups[#groups]
    if g and (h.lo < g.hi or (h.lo == h.hi and g.lo == g.hi and h.lo == g.lo)) then
      g.hi = math.max(g.hi, h.hi)
      table.insert(g.hunks, h)
    else
      table.insert(groups, { lo = h.lo, hi = h.hi, hunks = { h } })
    end
  end
  local out, at, clashes = {}, 0, 0
  local function copy(to)
    for i = at + 1, to do table.insert(out, from[i]) end
  end
  -- A group's lines as one side has them.
  local function side_of(g, side)
    local lines, pos = {}, g.lo
    for _, h in ipairs(g.hunks) do
      if h.side == side then
        for i = pos + 1, h.lo do table.insert(lines, from[i]) end
        vim.list_extend(lines, h.lines)
        pos = h.hi
      end
    end
    for i = pos + 1, g.hi do table.insert(lines, from[i]) end
    return lines
  end
  for _, g in ipairs(groups) do
    copy(g.lo)
    local mine, other = side_of(g, "ours"), side_of(g, "theirs")
    local sides = {}
    for _, h in ipairs(g.hunks) do sides[h.side] = true end
    if sides.ours and sides.theirs and not vim.deep_equal(mine, other) then
      clashes = clashes + 1
      vim.list_extend(out, mine)
      vim.list_extend(out, other)
    else
      vim.list_extend(out, sides.ours and mine or other)
    end
    at = g.hi
  end
  copy(#from)
  return #out > 0 and table.concat(out, "\n") .. "\n" or "", clashes
end

-- Puts `text` in the buffer by changing only the lines that differ, so the
-- cursor, the view and undo stay where they are.
function M.put(buf, text)
  local current = text_of(buf)
  if current == text then return end
  local changes = hunks(current, text)
  for i = #changes, 1, -1 do
    local h = changes[i]
    vim.api.nvim_buf_set_lines(buf, h.lo, h.hi, false, h.lines)
  end
end

function M.track(buf)
  base[buf] = read(vim.api.nvim_buf_get_name(buf))
  vim.api.nvim_create_autocmd("BufWipeout", { buffer = buf, once = true, callback = function() base[buf] = nil end })
end

-- After the app writes the note.
function M.written(buf)
  base[buf] = read(vim.api.nvim_buf_get_name(buf))
  vim.b[buf].notes_gone = nil
end

-- Brings in a change made outside, if there is one, and calls `changed` with
-- the buffer and the rows (1-based) the change wrote. Returns false when the
-- note's file has gone (deleted or moved outside the app), so the caller
-- doesn't write it back under its old name; it says so once.
function M.check(buf, changed)
  local path = vim.api.nvim_buf_get_name(buf)
  local disk = read(path)
  if not disk then
    if not vim.b[buf].notes_gone then
      vim.b[buf].notes_gone = true
      vim.notify(("notes: %s was moved or deleted outside Notes, so it isn't saved there any more"):format(vim.fs.basename(path)), vim.log.levels.WARN)
    end
    return false
  end
  vim.b[buf].notes_gone = nil
  if base[buf] == nil or disk == base[buf] then return true end
  local before = text_of(buf)
  if not vim.bo[buf].modified then
    M.put(buf, disk)
    vim.bo[buf].modified = false
  else
    local merged, clashes = M.merge(base[buf], text_of(buf), disk)
    M.put(buf, merged)
    if clashes > 0 then
      vim.notify(("notes: %s was changed outside Notes where you were typing; both versions are kept, yours first"):format(vim.fs.basename(path)), vim.log.levels.WARN)
    end
  end
  base[buf] = disk
  local rows = {}
  for _, h in ipairs(vim.diff(before, text_of(buf), { result_type = "indices" })) do
    for row = h[3], h[3] + h[4] - 1 do table.insert(rows, row) end
  end
  if changed then changed(buf, rows) end
  return true
end

-- Calls `on_change` soon after anything in `root` changes, and when Neovim
-- gets the focus back. Returns a function that stops it.
function M.watch(root, on_change)
  local pending = false
  local function soon()
    if pending then return end
    pending = true
    vim.defer_fn(function()
      pending = false
      on_change()
    end, 100)
  end
  local event = uv.new_fs_event()
  local ok = event and pcall(event.start, event, root, { recursive = true }, function() vim.schedule(soon) end)
  local focus = vim.api.nvim_create_autocmd("FocusGained", { callback = soon })
  return function()
    if ok then event:stop() end
    pcall(vim.api.nvim_del_autocmd, focus)
  end
end

return M
