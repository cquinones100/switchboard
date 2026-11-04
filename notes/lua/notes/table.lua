-- Tables, as Markdown writes them:
--
--   | Item | Qty |
--   | ---- | --- |
--   | Eggs | 12  |
--
-- drawn with box lines (│ ├ ┼ ┤ ─), the first row bold, and kept aligned:
-- leaving Insert mode, or any change in Normal mode, pads every cell of the
-- table the cursor is in to its column's width, measured as the cell shows
-- (concealed markup doesn't count). In Insert mode, Tab moves to the next
-- cell (past the last, it adds a row) and Shift-Tab to the one before.

local M = {}

local function is_row(line)
  return line ~= nil and line:match("^%s*|") ~= nil
end

local function is_separator(line)
  return line:match("^%s*|[%s:|%-]+|?%s*$") ~= nil and line:find("-", 1, true) ~= nil
end

-- The 1-based positions of a row's bars.
local function bars_of(line)
  local bars = {}
  for i = 1, #line do
    if line:sub(i, i) == "|" and line:sub(i - 1, i - 1) ~= "\\" then table.insert(bars, i) end
  end
  return bars
end

-- A row's cells, split on | that isn't escaped, without the outer bars.
function M.cells(line)
  local body = line:gsub("^%s*|", ""):gsub("|%s*$", "")
  local cells, current, i = {}, "", 1
  while i <= #body do
    local ch = body:sub(i, i)
    if ch == "\\" and body:sub(i + 1, i + 1) == "|" then
      current = current .. "\\|"
      i = i + 2
    elseif ch == "|" then
      table.insert(cells, vim.trim(current))
      current = ""
      i = i + 1
    else
      current = current .. ch
      i = i + 1
    end
  end
  table.insert(cells, vim.trim(current))
  return cells
end

-- How wide a cell shows: its width less the markup concealed in it.
local function shown_width(cell)
  local hidden = 0
  for _, span in ipairs(require("notes.render").inline(cell)) do
    for _, c in ipairs(span.conceal) do hidden = hidden + (c[2] - c[1]) end
  end
  return vim.fn.strdisplaywidth(cell) - hidden
end

-- The first and last row (1-based) of the table around `row`, or nil.
function M.bounds(lines, row)
  if not is_row(lines[row]) then return nil end
  local first, last = row, row
  while is_row(lines[first - 1]) do first = first - 1 end
  while is_row(lines[last + 1]) do last = last + 1 end
  return first, last
end

-- A table's rows, padded so every column lines up. Separator rows keep their
-- alignment colons.
function M.aligned(rows)
  local indent = rows[1]:match("^(%s*)")
  local parsed, widths, columns = {}, {}, 0
  for i, line in ipairs(rows) do
    parsed[i] = { separator = is_separator(line), cells = M.cells(line) }
    columns = math.max(columns, #parsed[i].cells)
  end
  for c = 1, columns do widths[c] = 3 end
  for _, row in ipairs(parsed) do
    if not row.separator then
      for c, cell in ipairs(row.cells) do widths[c] = math.max(widths[c], shown_width(cell)) end
    end
  end
  local out = {}
  for i, row in ipairs(parsed) do
    local parts = {}
    for c = 1, columns do
      local cell = row.cells[c] or ""
      if row.separator then
        local left, right = cell:match("^:") ~= nil, cell:match(":$") ~= nil
        local dashes = string.rep("-", widths[c] - (left and 1 or 0) - (right and 1 or 0))
        parts[c] = (left and ":" or "") .. dashes .. (right and ":" or "")
      else
        parts[c] = cell .. string.rep(" ", widths[c] - shown_width(cell))
      end
    end
    out[i] = indent .. "| " .. table.concat(parts, " | ") .. " |"
  end
  return out
end

-- Aligns the table the cursor is in, as one change with the edit before it.
function M.align(buf, row)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local first, last = M.bounds(lines, row)
  if not first then return end
  local rows = vim.list_slice(lines, first, last)
  local aligned = M.aligned(rows)
  if vim.deep_equal(rows, aligned) then return end
  pcall(vim.cmd, "undojoin")
  vim.api.nvim_buf_set_lines(buf, first - 1, last, false, aligned)
end

-- Aligns every table in a note, for one just opened.
function M.align_all(buf)
  local row = 1
  while row <= vim.api.nvim_buf_line_count(buf) do
    local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    local _, last = M.bounds(lines, row)
    if last then
      local rows = vim.list_slice(lines, row, last)
      local aligned = M.aligned(rows)
      if not vim.deep_equal(rows, aligned) then vim.api.nvim_buf_set_lines(buf, row - 1, last, false, aligned) end
      row = last + 1
    else
      row = row + 1
    end
  end
end

-- ── Drawing (called by render.draw for each table line) ────────────────────

function M.draw(buf, row, line, header, mark)
  local separator = is_separator(line)
  local bars = vim.tbl_map(function(b) return b - 1 end, bars_of(line))
  for n, col in ipairs(bars) do
    local char = "│"
    if separator then char = n == 1 and "├" or n == #bars and "┤" or "┼" end
    mark(buf, row, col, { end_col = col + 1, conceal = char, hl_group = "NotesTableBorder", priority = 210 })
  end
  if separator then
    for i = 1, #line do
      local ch = line:sub(i, i)
      if ch == "-" or ch == ":" then mark(buf, row, i - 1, { end_col = i, conceal = "─", hl_group = "NotesTableBorder", priority = 210 }) end
      if ch == " " and i > 1 and i < #line then mark(buf, row, i - 1, { end_col = i, conceal = "─", hl_group = "NotesTableBorder", priority = 205 }) end
    end
  elseif header and #bars > 1 then
    mark(buf, row, bars[1] + 1, { end_col = bars[#bars], hl_group = "NotesTableHeader", priority = 155 })
  end
end

-- ── Editing ────────────────────────────────────────────────────────────────

-- A new table under the cursor's line, two columns by two rows, with the
-- cursor in its first cell.
function M.insert(buf)
  local row = vim.api.nvim_win_get_cursor(0)[1]
  local lines = { "|     |     |", "| --- | --- |", "|     |     |" }
  local current = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""
  local at = row
  if current:match("%S") then
    table.insert(lines, 1, "")
  else
    at = row - 1
  end
  vim.api.nvim_buf_set_lines(buf, at, at, false, lines)
  local first = at + (current:match("%S") and 2 or 1)
  vim.api.nvim_win_set_cursor(0, { first, 2 })
  vim.cmd("startinsert")
end

-- The 0-based columns where each cell's text starts on a row: its first
-- character, or just past "| " in an empty cell.
local function cell_starts(line)
  local bars, starts = bars_of(line), {}
  for n = 1, #bars - 1 do
    local s = bars[n] + 1
    while line:sub(s, s) == " " and s < bars[n + 1] do s = s + 1 end
    starts[n] = s < bars[n + 1] and s - 1 or bars[n] + 1
  end
  return starts
end

-- Which cell (1-based) a 0-based column is in, or 0 before the first bar.
local function cell_index(line, col)
  local index = 0
  for n, bar in ipairs(bars_of(line)) do
    if col >= bar - 1 then index = n end
  end
  return math.min(index, math.max(#bars_of(line) - 1, 0))
end

-- Moves to the next (step 1) or previous (step -1) cell, aligning first. Past
-- the last cell of the last row, adds a row.
function M.jump(buf, step)
  local row = vim.api.nvim_win_get_cursor(0)[1]
  M.align(buf, row)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local first, last = M.bounds(lines, row)
  if not first then return false end
  local col = vim.api.nvim_win_get_cursor(0)[2]
  local r = row
  local starts = cell_starts(lines[r])
  local index = cell_index(lines[r], col) + step
  while true do
    if index >= 1 and index <= #starts and not is_separator(lines[r]) then break end
    r = r + step
    if r < first then return true end
    if r > last then
      M.add_row(buf, last)
      lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
      last = last + 1
    end
    starts = cell_starts(lines[r])
    index = step > 0 and 1 or #starts
  end
  vim.api.nvim_win_set_cursor(0, { r, starts[index] })
  return true
end

-- An empty row under `row`, with as many cells as the table has.
function M.add_row(buf, row)
  local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1]
  local count = #M.cells(line)
  local indent = line:match("^(%s*)")
  vim.api.nvim_buf_set_lines(buf, row, row, false, { indent .. "|" .. string.rep("     |", count) })
  M.align(buf, row)
end

-- An empty column after the one the cursor is in.
function M.add_column(buf)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local first, last = M.bounds(lines, row)
  if not first then return end
  local at = cell_index(lines[row], col)
  local out = {}
  for r = first, last do
    local cells = M.cells(lines[r])
    table.insert(cells, at + 1, is_separator(lines[r]) and "---" or "")
    out[#out + 1] = lines[r]:match("^(%s*)") .. "| " .. table.concat(cells, " | ") .. " |"
  end
  vim.api.nvim_buf_set_lines(buf, first - 1, last, false, M.aligned(out))
end

function M.delete_row(buf)
  local row = vim.api.nvim_win_get_cursor(0)[1]
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  if not M.bounds(lines, row) or is_separator(lines[row]) then return end
  vim.api.nvim_buf_set_lines(buf, row - 1, row, false, {})
end

function M.delete_column(buf)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local first, last = M.bounds(lines, row)
  if not first then return end
  local at = math.max(cell_index(lines[row], col), 1)
  local out = {}
  for r = first, last do
    local cells = M.cells(lines[r])
    table.remove(cells, at)
    if #cells == 0 then
      vim.api.nvim_buf_set_lines(buf, first - 1, last, false, {})
      return
    end
    out[#out + 1] = lines[r]:match("^(%s*)") .. "| " .. table.concat(cells, " | ") .. " |"
  end
  vim.api.nvim_buf_set_lines(buf, first - 1, last, false, M.aligned(out))
end

M.is_row = is_row
M.is_separator = is_separator

return M
