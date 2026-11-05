-- The editing side of Notes' Format menu, and the typing it does for you.
--
-- Paragraph styles change what a line starts with (see render.lua for what
-- each looks like); applying the style a line already has makes it body
-- text again, as Notes does. Character styles wrap the selection, or the
-- word under the cursor, in their markers, and take them off again when they
-- are there already. Lists carry on when you press Enter in one, and stop
-- when you press it on an empty item; Tab and Shift-Tab indent and outdent an
-- item while you type.

local M = {}

local render = require("notes.render")

-- Notes' "Move checked items to bottom": when on, ticking an item moves it
-- under the ones still to do, so a list leads with what is left. On by
-- default; turn it off with
-- `require("notes").setup({ move_checked_to_bottom = false })`.
-- With it, a blank line parts the ticked items from the rest
-- (`gap_before_checked`), so the list's two halves read apart.
M.options = { move_checked_to_bottom = true, gap_before_checked = true }

local function get_line(buf, row)
  return vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""
end

local function set_line(buf, row, text)
  vim.api.nvim_buf_set_lines(buf, row - 1, row, false, { text })
end

-- A line's indent, the list or heading marker after it (with its space), and
-- the text after that.
function M.split(line)
  local indent, rest = line:match("^(%s*)(.*)$")
  for _, pattern in ipairs({
    "^(#+%s+)",
    "^([-*+]%s+%[[ xX]%]%s+)",
    "^([-*+]%s+%[[ xX]%])$",
    "^([-*+]%s+)",
    "^(%d+[.)]%s+)",
    "^(>%s?)",
  }) do
    local marker = rest:match(pattern)
    if marker then return indent, marker, rest:sub(#marker + 1) end
  end
  return indent, "", rest
end

-- The marker each paragraph style starts a line with.
M.PARAGRAPH = {
  title = "# ",
  heading = "## ",
  subheading = "### ",
  body = "",
  bullet = "* ",
  dash = "- ",
  number = "1. ",
  check = "- [ ] ",
  quote = "> ",
}

local function style_of(marker)
  if marker:match("^#%s") then return "title" end
  if marker:match("^##%s") then return "heading" end
  if marker:match("^###+%s") then return "subheading" end
  if marker:match("%[[ xX]%]") then return "check" end
  if marker:match("^[*+]") then return "bullet" end
  if marker:match("^%-") then return "dash" end
  if marker:match("^%d") then return "number" end
  if marker:match("^>") then return "quote" end
  return "body"
end

-- Gives lines first..last `style`, or body text when the first already has
-- it. A numbered list is numbered from 1. Headings have no indent.
function M.paragraph(buf, first, last, style)
  local _, first_marker = M.split(get_line(buf, first))
  if style_of(first_marker) == style then style = "body" end
  local n = 0
  for row = first, last do
    local indent, _, text = M.split(get_line(buf, row))
    local marker = M.PARAGRAPH[style]
    if style == "number" then
      n = n + 1
      marker = n .. ". "
    end
    if style == "title" or style == "heading" or style == "subheading" then indent = "" end
    set_line(buf, row, indent .. marker .. text)
  end
  -- A cursor left on the marker goes to the text, so typing starts there.
  local win = vim.fn.bufwinid(buf)
  if win ~= -1 then
    local row, col = unpack(vim.api.nvim_win_get_cursor(win))
    if row >= first and row <= last then
      local line = get_line(buf, row)
      local start = M.text_start(line)
      if col < start then vim.api.nvim_win_set_cursor(win, { row, math.min(start, math.max(#line - 1, 0)) }) end
    end
  end
end

-- Where a line's text starts (0-based): after its indent and its list,
-- checklist, quote or heading marker.
function M.text_start(line)
  local indent, marker = M.split(line)
  return #indent + #marker
end

-- Where i, a or I starts typing on a line with a marker, which shows as one
-- symbol (☐, •, ▎) or not at all (#) but is several characters in the file:
-- never inside it. Returns the 0-based column to type at, or nil for the
-- key to do what it always does.
function M.insert_point(line, col, key)
  local start = M.text_start(line)
  if start == 0 then return nil end
  if key == "I" then return start end
  if key == "i" and col < start then return start end
  if key == "a" and col + 1 < start then return start end
  return nil
end

-- Monospaced: the lines go between ``` fences, or come out of the block
-- they are in.
function M.monospaced(buf, first, last)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local kinds = render.kinds(lines)
  local k = kinds[first].kind
  if k == "code" or k == "fence" then
    local open, close = first, first
    while open > 1 and kinds[open].kind ~= "fence" do open = open - 1 end
    if k == "fence" and kinds[first + 1] and kinds[first + 1].kind == "code" then open = first end
    close = open + 1
    while close <= #lines and kinds[close].kind ~= "fence" do close = close + 1 end
    vim.api.nvim_buf_set_lines(buf, close - 1, close, false, {})
    vim.api.nvim_buf_set_lines(buf, open - 1, open, false, {})
    return
  end
  vim.api.nvim_buf_set_lines(buf, last, last, false, { "```" })
  vim.api.nvim_buf_set_lines(buf, first - 1, first - 1, false, { "```" })
end

-- ── Character styles ───────────────────────────────────────────────────────

M.CHARACTER = {
  bold = { "**", "**" },
  italic = { "*", "*" },
  underline = { "<u>", "</u>" },
  strike = { "~~", "~~" },
  highlight = { "==", "==" },
  code = { "`", "`" },
}

-- Wraps columns s..e (0-based, e exclusive) of a row in a style's markers,
-- or unwraps them when they are wrapped already.
function M.wrap(buf, row, s, e, style)
  local open, close = unpack(M.CHARACTER[style])
  local line = get_line(buf, row)
  local before, inside, after = line:sub(1, s), line:sub(s + 1, e), line:sub(e + 1)
  -- A one-character marker that is part of a longer run, as the * of **bold**
  -- is, isn't this style's.
  local single = #open == 1 and (before:sub(-2, -2) == open or after:sub(2, 2) == close)
  if before:sub(-#open) == open and after:sub(1, #close) == close and not single then
    set_line(buf, row, before:sub(1, -#open - 1) .. inside .. after:sub(#close + 1))
    return s - #open
  end
  if inside:sub(1, #open) == open and inside:sub(-#close) == close and #inside >= #open + #close then
    set_line(buf, row, before .. inside:sub(#open + 1, -#close - 1) .. after)
    return s
  end
  set_line(buf, row, before .. open .. inside .. close .. after)
  return s + #open
end

-- The word under the cursor, as 0-based columns, e exclusive.
local function word_at(line, col)
  local s, e = col + 1, col + 1
  while s > 1 and line:sub(s - 1, s - 1):match("[%w_'%-]") do s = s - 1 end
  while e <= #line and line:sub(e, e):match("[%w_'%-]") do e = e + 1 end
  return s - 1, e - 1
end

-- The highlight render.inline gives each style's text, to find it by.
local STYLE_HL = { bold = "NotesBold", italic = "NotesItalic", underline = "NotesUnderline", strike = "NotesStrike", highlight = "NotesHighlight", code = "NotesMono" }

-- An empty pair of markers put in for you to type into (see M.character),
-- taken out again if you leave Insert mode without typing anything in it.
M.pending = nil

-- Takes the empty pair out, if it is still there and still empty.
function M.drop_empty(buf)
  local p = M.pending
  M.pending = nil
  if not p or p.buf ~= buf then return end
  local line = get_line(buf, p.row)
  if line:sub(p.col + 1, p.col + #p.open + #p.close) == p.open .. p.close then
    set_line(buf, p.row, line:sub(1, p.col) .. line:sub(p.col + #p.open + #p.close + 1))
  end
end

-- A character style, as Notes applies one:
--   on the visual selection (charwise, or every line of a linewise one, each
--     from its text's start);
--   with the cursor in text that has the style already, taking it off that
--     whole run of text;
--   with the cursor on a word, on that word;
--   anywhere else (a space, an empty line, just after a word you typed), on
--     what you type next: its markers go in at the cursor, and Insert mode
--     starts between them.
-- `typing_col` is where you were typing when this came from Insert mode
-- (the Format menu's Ctrl-P), which then carries on in Insert mode.
function M.character(buf, style, visual, typing_col)
  if visual then
    local mode = vim.fn.visualmode()
    local s, e = vim.fn.getpos("'<"), vim.fn.getpos("'>")
    for row = s[2], e[2] do
      local line = get_line(buf, row)
      local indent, marker = M.split(line)
      local from = (mode == "v" and row == s[2]) and s[3] - 1 or #indent + #marker
      local to = (mode == "v" and row == e[2]) and math.min(e[3], #line) or #line
      if to > from then M.wrap(buf, row, from, to, style) end
    end
    return
  end
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = get_line(buf, row)
  local caret = typing_col or col
  local function carry_on(at)
    vim.api.nvim_win_set_cursor(0, { row, at })
    if typing_col then vim.cmd(at >= #get_line(buf, row) and "startinsert!" or "startinsert") end
  end
  -- In text that has the style: off, for the whole run.
  for _, span in ipairs(render.inline(line)) do
    local open, close = span.conceal[1], span.conceal[#span.conceal]
    if span.hl == STYLE_HL[style] and #span.conceal == 2 and caret >= open[1] and caret < close[2] then
      set_line(buf, row, line:sub(1, open[1]) .. line:sub(open[2] + 1, close[1]) .. line:sub(close[2] + 1))
      -- The cursor stays on the same text: the opening marker is gone from
      -- before it, and a cursor that was on a marker goes to the text's edge.
      local opened = open[2] - open[1]
      local at = caret < open[2] and open[1] or caret <= close[1] and caret - opened or close[1] - opened
      return carry_on(at)
    end
  end
  -- On a word: that word. Typing, the cursor has to be inside it, not just
  -- after it, since that is where the next letter goes.
  local is_word = function(ch) return ch ~= "" and ch:match("[%w_'%-]") ~= nil end
  local on_word = typing_col and (is_word(line:sub(caret, caret)) and is_word(line:sub(caret + 1, caret + 1))) or (not typing_col and is_word(line:sub(col + 1, col + 1)))
  if on_word then
    local s, e = word_at(line, typing_col and caret - 1 or col)
    local open = M.CHARACTER[style][1]
    local at = M.wrap(buf, row, s, e, style)
    return carry_on(typing_col and (caret + (at - s)) or at)
  end
  -- Anywhere else: what you type next.
  local open, close = unpack(M.CHARACTER[style])
  local at = typing_col or (line == "" and 0 or col + 1)
  at = math.min(at, #line)
  set_line(buf, row, line:sub(1, at) .. open .. close .. line:sub(at + 1))
  M.pending = { buf = buf, row = row, col = at, open = open, close = close }
  vim.api.nvim_win_set_cursor(0, { row, at + #open })
  vim.cmd("startinsert")
end

-- A link on the selection or the word under the cursor, to an address you
-- type; with nothing under the cursor, the address is the link's text too.
function M.link(buf, visual)
  vim.ui.input({ prompt = "Link to: ", default = "https://" }, function(url)
    if not url or url == "" or url == "https://" then return end
    local row, s, e
    if visual then
      local a, b = vim.fn.getpos("'<"), vim.fn.getpos("'>")
      row, s, e = a[2], a[3] - 1, (a[2] == b[2]) and math.min(b[3], #get_line(buf, a[2])) or #get_line(buf, a[2])
    else
      row = vim.api.nvim_win_get_cursor(0)[1]
      s, e = word_at(get_line(buf, row), vim.api.nvim_win_get_cursor(0)[2])
    end
    local line = get_line(buf, row)
    local text = e > s and line:sub(s + 1, e) or url
    set_line(buf, row, line:sub(1, s) .. "[" .. text .. "](" .. url .. ")" .. line:sub(e + 1))
  end)
end

-- ── Checklists ─────────────────────────────────────────────────────────────

local function indent_of(line) return #line:match("^%s*") end
local function is_check(line) return render.line_kind(line).kind == "check" end

-- The rows of the checklist around `row`: the run of checklist items either
-- side of it, with whatever is indented under them (an item's details and
-- sub-items), starting and ending on an item. One blank line before ticked
-- items is the gap sorted leaves, and the list goes on across it.
local function checklist_bounds(buf, row)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local function part(r)
    local line = lines[r]
    return line ~= nil and (is_check(line) or (line:match("%S") ~= nil and indent_of(line) > 0))
  end
  if not part(row) then return nil end
  local function blank(r) return lines[r] ~= nil and lines[r]:match("%S") == nil end
  local function ticked(r) return lines[r] ~= nil and render.line_kind(lines[r]).done == true end
  local first, last = row, row
  local grew = true
  while grew do
    grew = false
    while part(first - 1) do first, grew = first - 1, true end
    while part(last + 1) do last, grew = last + 1, true end
    if blank(last + 1) and ticked(last + 2) then
      last, grew = last + 2, true
    end
    if ticked(first) and blank(first - 1) and part(first - 2) then first, grew = first - 2, true end
  end
  while first <= last and not is_check(lines[first]) do first = first + 1 end
  -- What is indented under the last item (an image pasted under it, say) is
  -- the item's, and moves with it.
  while last >= first and not is_check(lines[last]) and indent_of(lines[last]) == 0 do last = last - 1 end
  if first > last then return nil end
  return first, last
end

-- Lines of a list, ticked items under the rest at every level: each item
-- moves with everything indented under it, and each group keeps its order.
-- Lines before the first item at the shallowest level stay first.
local function sorted(lines, top)
  if top then lines = vim.tbl_filter(function(line) return line:match("%S") ~= nil end, lines) end
  local base
  for _, line in ipairs(lines) do
    if is_check(line) and (not base or indent_of(line) < base) then base = indent_of(line) end
  end
  if not base then return lines end
  local head, items = {}, {}
  for _, line in ipairs(lines) do
    if is_check(line) and indent_of(line) <= base then
      table.insert(items, { line })
    elseif #items > 0 then
      table.insert(items[#items], line)
    else
      table.insert(head, line)
    end
  end
  local todo, done = {}, {}
  for _, item in ipairs(items) do
    local under = sorted(vim.list_slice(item, 2))
    local whole = vim.list_extend({ item[1] }, under)
    table.insert(render.line_kind(item[1]).done and done or todo, whole)
  end
  local out = head
  for _, group in ipairs(todo) do vim.list_extend(out, group) end
  if top and M.options.gap_before_checked and #todo > 0 and #done > 0 then table.insert(out, "") end
  for _, group in ipairs(done) do vim.list_extend(out, group) end
  return out
end

-- Moves the ticked items of the checklist around `row` under the rest, as
-- `sorted` does.
function M.sort_checked(buf, row)
  local first, last = checklist_bounds(buf, row)
  if not first then return end
  local lines = vim.api.nvim_buf_get_lines(buf, first - 1, last, false)
  local out = sorted(lines, true)
  if not vim.deep_equal(out, lines) then vim.api.nvim_buf_set_lines(buf, first - 1, last, false, out) end
end

-- Ticks or unticks the item on `row`. Returns whether it was an item.
function M.toggle_check(buf, row)
  local line = get_line(buf, row)
  local kind = render.line_kind(line)
  if kind.kind ~= "check" then return false end
  local box = line:find("[", kind.indent + 1, true)
  local mark = line:sub(box + 1, box + 1) == " " and "x" or " "
  set_line(buf, row, line:sub(1, box) .. mark .. line:sub(box + 2))
  if M.options.move_checked_to_bottom then M.sort_checked(buf, row) end
  return true
end

local function each_check(buf, fn)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local kinds = render.kinds(lines)
  for row = #lines, 1, -1 do
    if kinds[row].kind == "check" then fn(row, lines[row], kinds[row]) end
  end
end

-- Takes out the blank lines sorted left before ticked items, bottom up, for
-- a change that leaves no ticked items there.
local function close_gaps(buf)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  for row = #lines - 1, 2, -1 do
    local above, below = lines[row - 1], lines[row + 1]
    if lines[row]:match("%S") == nil and render.line_kind(below).done == true and (is_check(above) or (above:match("%S") and indent_of(above) > 0)) then
      vim.api.nvim_buf_set_lines(buf, row - 1, row, false, {})
    end
  end
end

-- Notes' Uncheck All and Delete Checked Items, for the whole note.
function M.uncheck_all(buf)
  close_gaps(buf)
  each_check(buf, function(row, line, kind)
    if kind.done then
      local box = line:find("[", kind.indent + 1, true)
      set_line(buf, row, line:sub(1, box) .. " " .. line:sub(box + 2))
    end
  end)
end

function M.delete_checked(buf)
  close_gaps(buf)
  -- Each ticked item with what is indented under it (an image pasted there,
  -- say), bottom up, so the rows above stay where they are.
  each_check(buf, function(row, line, kind)
    if not kind.done then return end
    local lines = vim.api.nvim_buf_get_lines(buf, row, -1, false)
    local under = 0
    while lines[under + 1] and lines[under + 1]:match("%S") and indent_of(lines[under + 1]) > indent_of(line) and not is_check(lines[under + 1]) do under = under + 1 end
    vim.api.nvim_buf_set_lines(buf, row - 1, row + under, false, {})
  end)
end

-- ── Typing ─────────────────────────────────────────────────────────────────

-- Enter in Insert mode: in a list, a new item of the same kind (numbered on,
-- unticked), or, on an empty item, the end of the list. Returns false when the
-- line isn't a list item, for Enter to do what it always does.
function M.enter(buf)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = get_line(buf, row)
  local kind = render.line_kind(line)
  if not vim.tbl_contains({ "check", "bullet", "dash", "number", "quote" }, kind.kind) then return false end
  local indent, marker, text = M.split(line)
  if vim.trim(text) == "" then
    set_line(buf, row, "")
    vim.api.nvim_win_set_cursor(0, { row, 0 })
    return true
  end
  local next_marker = marker
  if kind.kind == "check" then next_marker = marker:gsub("%[[xX]%]", "[ ]") end
  if kind.kind == "number" then
    local n, sep = marker:match("^(%d+)([.)])")
    next_marker = (tonumber(n) + 1) .. sep .. " "
  end
  local before, after = line:sub(1, col), line:sub(col + 1)
  local prefix = indent .. next_marker
  vim.api.nvim_buf_set_lines(buf, row - 1, row, false, { before, prefix .. vim.trim(after) })
  vim.api.nvim_win_set_cursor(0, { row + 1, #prefix })
  if kind.kind == "number" then M.renumber(buf, row + 1) end
  return true
end

-- Numbers the numbered list around `row` 1, 2, 3… at each indent.
function M.renumber(buf, row)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local kinds = render.kinds(lines)
  if kinds[row].kind ~= "number" then return end
  local first = row
  while kinds[first - 1] and (kinds[first - 1].kind == "number" or (lines[first - 1]:match("^%s+%S") and kinds[first - 1].kind ~= "body")) do first = first - 1 end
  local counts = {}
  for r = first, #lines do
    local k = kinds[r]
    if k.kind ~= "number" and not (lines[r]:match("^%s+%S") and k.kind ~= "body" and k.kind ~= "title") then break end
    if k.kind == "number" then
      counts[k.indent] = (counts[k.indent] or 0) + 1
      for indent in pairs(counts) do
        if indent > k.indent then counts[indent] = nil end
      end
      local renumbered = lines[r]:gsub("^(%s*)%d+", "%1" .. counts[k.indent], 1)
      if renumbered ~= lines[r] then set_line(buf, r, renumbered) end
    end
  end
end

-- Tab and Shift-Tab in Insert mode: a table's next or previous cell, or a
-- list item indented or outdented by a shiftwidth. False otherwise.
function M.tab(buf, step)
  local row, col = unpack(vim.api.nvim_win_get_cursor(0))
  local line = get_line(buf, row)
  local kind = render.line_kind(line)
  if kind.kind == "table" then return require("notes.table").jump(buf, step) end
  if not vim.tbl_contains({ "check", "bullet", "dash", "number" }, kind.kind) then return false end
  local width = vim.fn.shiftwidth()
  if step > 0 then
    set_line(buf, row, string.rep(" ", width) .. line)
    vim.api.nvim_win_set_cursor(0, { row, col + width })
  else
    local removed = math.min(width, #line:match("^(%s*)"))
    if removed == 0 then return true end
    set_line(buf, row, line:sub(removed + 1))
    vim.api.nvim_win_set_cursor(0, { row, math.max(col - removed, 0) })
  end
  if kind.kind == "number" then M.renumber(buf, row) end
  return true
end

return M
