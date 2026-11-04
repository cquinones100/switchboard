-- Draws a note the way Notes shows it, over its Markdown, with extmarks: the
-- text stays plain Markdown, and only what is shown changes.
--
-- Paragraph styles, from each line's start:
--   # Title            ## Heading          ### Subheading
--   * bulleted (•)     - dashed (–)        1. numbered
--   - [ ] checklist (☐, ☑ once ticked, its text dimmed)
--   > block quote (▎)  ```  monospaced, between fences
--   | a | b |          a table (see table.lua for keeping it aligned)
-- and inside a line, outside `code`:
--   **bold**  *italic*  <u>underline</u>  ~~strikethrough~~  ==highlight==
--   [text](url) and bare https:// links, [[Note title]] links to notes,
--   #tags
--
-- Markup is concealed, so it shows only in Insert mode on the line being
-- typed (concealcursor "nc"), the way Notes shows formatting rather than
-- symbols. Headings get a ▾ to fold their section by (see fold.lua).

local M = {}

-- A ticked item, and everything indented under it, struck through as well as
-- dimmed; off, only dimmed.
M.options = { strike_checked = true }

M.ns = vim.api.nvim_create_namespace("notes.render")

local store = require("notes.store")

-- ── Highlights ─────────────────────────────────────────────────────────────

-- From the colorscheme, so notes look at home in it: each group takes the
-- foreground of a group every colorscheme has, and adds its own style.
local function from(group, extra)
  local ok, hl = pcall(vim.api.nvim_get_hl, 0, { name = group, link = false })
  local spec = vim.tbl_extend("force", ok and { fg = hl.fg } or {}, extra)
  return spec
end

function M.highlights()
  local set = function(name, spec) vim.api.nvim_set_hl(0, name, spec) end
  set("NotesTitle", from("Title", { bold = true, underline = true }))
  set("NotesHeading", from("Function", { bold = true }))
  set("NotesSubheading", from("Identifier", { bold = true }))
  set("NotesBold", { bold = true })
  set("NotesItalic", { italic = true })
  set("NotesUnderline", { underline = true })
  set("NotesStrike", { strikethrough = true })
  set("NotesHighlight", from("Normal", { bg = "#5c4f1d", fg = "#f5e9b8" }))
  set("NotesMono", from("String", {}))
  local ok, cursorline = pcall(vim.api.nvim_get_hl, 0, { name = "CursorLine", link = false })
  set("NotesMonoBlock", { bg = ok and cursorline.bg or nil })
  set("NotesFence", { link = "Comment" })
  set("NotesQuote", from("Comment", { italic = true }))
  set("NotesQuoteBar", from("Special", {}))
  set("NotesBullet", from("Special", { bold = true }))
  set("NotesBox", from("Special", {}))
  set("NotesBoxDone", from("DiagnosticOk", { bold = true }))
  set("NotesChecked", from("Comment", {}))
  set("NotesCheckedStrike", from("Comment", { strikethrough = true }))
  set("NotesLink", from("Function", { underline = true }))
  set("NotesNoteLink", from("Constant", { underline = true }))
  set("NotesNoteLinkMissing", from("Comment", { underline = true, italic = true }))
  set("NotesTag", from("Type", { bold = true }))
  set("NotesTableBorder", from("Comment", {}))
  set("NotesTableHeader", { bold = true })
  set("NotesFold", from("Special", {}))
end

-- ── Block styles ───────────────────────────────────────────────────────────

-- What a line is, as { kind, ... }. `in_code` is whether it is between
-- fences, which the caller tracks from the lines before it.
function M.line_kind(line, first_text_line)
  local indent, rest = line:match("^(%s*)(.*)$")
  local fence = rest:match("^```")
  if fence then return { kind = "fence" } end
  local hashes, after = rest:match("^(#+)%s+(.*)$")
  if hashes and indent == "" then
    local level = #hashes
    local kind = (level == 1 or first_text_line) and "title" or level == 2 and "heading" or "subheading"
    return { kind = kind, level = level, marker = #hashes + 1 }
  end
  local box, mark = rest:match("^([-*+]%s+%[([ xX])%])%s")
  if box then
    return { kind = "check", indent = #indent, done = mark ~= " ", marker = #box }
  end
  local bullet = rest:match("^([*+])%s")
  if bullet then return { kind = "bullet", indent = #indent } end
  if rest:match("^%-%s") then return { kind = "dash", indent = #indent } end
  local number = rest:match("^(%d+[.)])%s")
  if number then return { kind = "number", indent = #indent, marker = #number } end
  if rest:match("^>%s?") then return { kind = "quote", indent = #indent } end
  if rest:match("^|") then return { kind = "table", indent = #indent } end
  -- A note's first line is its title, written as one or not.
  if first_text_line then return { kind = "title", level = 1, marker = 0 } end
  return { kind = "body" }
end

-- ── Inline styles ──────────────────────────────────────────────────────────

-- The spans of inline styles in a line, as { s, e, hl, conceal = {{s, e}…} }
-- with 0-based byte columns, e exclusive. Each construct found blanks its
-- markers in `masked`, so a later one can't match them: `code` first (its
-- insides are never styled), then links, then the styles, which may nest.
function M.inline(line, from_col)
  from_col = from_col or 0
  local spans = {}
  local masked = line
  local function blank(s, e) -- 1-based inclusive
    masked = masked:sub(1, s - 1) .. string.rep("\1", e - s + 1) .. masked:sub(e + 1)
  end
  -- fn returns false for a match it turns down, and the search goes on from
  -- the next character rather than past the match.
  local function each(pattern, fn)
    local init = from_col + 1
    while true do
      local s, e, a, b = masked:find(pattern, init)
      if not s then break end
      init = fn(s, e, a, b) == false and s + 1 or e + 1
    end
  end
  -- `code`
  each("`([^`]+)`", function(s, e)
    table.insert(spans, { s = s, e = e - 1, hl = "NotesMono", conceal = { { s - 1, s }, { e - 1, e } } })
    blank(s, e)
  end)
  -- [[Note link]]
  each("%[%[([^%]]+)%]%]", function(s, e, title)
    table.insert(spans, { s = s + 1, e = e - 2, hl = "NotesNoteLink", note = title, conceal = { { s - 1, s + 1 }, { e - 2, e } } })
    blank(s, e)
  end)
  -- [text](url)
  each("%[([^%]]+)%]%(([^%)%s]+)%)", function(s, e, text, url)
    local close = s + #text + 1 -- the ]
    table.insert(spans, { s = s, e = close - 1, hl = "NotesLink", url = url, conceal = { { s - 1, s }, { close - 1, e } } })
    blank(s, s)
    blank(close, e)
  end)
  -- bare links
  each("https?://[%w%-%._~:/%?#%[%]@!%$&'%(%)%*%+,;=%%]+", function(s, e)
    local url = masked:sub(s, e):gsub("[.,;:!?%)]+$", "")
    table.insert(spans, { s = s - 1, e = s - 1 + #url, hl = "NotesLink", url = url, conceal = {} })
    blank(s, s + #url - 1)
  end)
  -- Paired markers: the text between gets the style, the markers are hidden.
  local function paired(open, close, hl, pattern)
    each(pattern, function(s, e)
      table.insert(spans, { s = s - 1 + #open, e = e - #close, hl = hl, conceal = { { s - 1, s - 1 + #open }, { e - #close, e } } })
      blank(s, s + #open - 1)
      blank(e - #close + 1, e)
    end)
  end
  paired("<u>", "</u>", "NotesUnderline", "<u>.-</u>")
  paired("~~", "~~", "NotesStrike", "~~[^~].-~~")
  paired("==", "==", "NotesHighlight", "==[^=].-==")
  paired("**", "**", "NotesBold", "%*%*[^%*%s].-%*%*")
  paired("__", "__", "NotesBold", "__[^_%s].-__")
  paired("*", "*", "NotesItalic", "%*[^%*%s][^%*]-%*")
  -- _italic_ only at word boundaries, so snake_case stays as it is.
  each("_[^_%s][^_]-_", function(s, e)
    local before, after = masked:sub(s - 1, s - 1), masked:sub(e + 1, e + 1)
    if before:match("[%w]") or after:match("[%w]") then return false end
    table.insert(spans, { s = s, e = e - 1, hl = "NotesItalic", conceal = { { s - 1, s }, { e - 1, e } } })
    blank(s, s)
    blank(e, e)
  end)
  -- #tags, as store.tags_in finds them.
  each("#[%w][%w_%-/]*", function(s, e)
    local before = s == 1 and " " or masked:sub(s - 1, s - 1)
    if not before:match("%s") then return false end
    table.insert(spans, { s = s - 1, e = e, hl = "NotesTag", tag = masked:sub(s + 1, e), conceal = {} })
  end)
  return spans
end

-- ── Drawing ────────────────────────────────────────────────────────────────

local function mark(buf, row, col, opts)
  pcall(vim.api.nvim_buf_set_extmark, buf, M.ns, row, col, opts)
end

local function conceal(buf, row, s, e, char)
  if e > s then mark(buf, row, s, { end_col = e, conceal = char or "", priority = 200 }) end
end

local function hl(buf, row, s, e, group, priority)
  if e > s then mark(buf, row, s, { end_col = e, hl_group = group, priority = priority or 150 }) end
end

-- What every line is, for the folds and the click handlers: { kind, ... } by
-- row, tracking fences.
function M.kinds(lines)
  local kinds, in_code, seen_text = {}, false, false
  for i, line in ipairs(lines) do
    local kind
    if in_code then
      kind = line:match("^%s*```") and { kind = "fence" } or { kind = "code" }
    else
      kind = M.line_kind(line, not seen_text and line:match("%S") ~= nil)
    end
    if kind.kind == "fence" then in_code = not in_code end
    if line:match("%S") then seen_text = true end
    kinds[i] = kind
  end
  return kinds
end

function M.draw(buf)
  if not vim.api.nvim_buf_is_valid(buf) then return end
  vim.api.nvim_buf_clear_namespace(buf, M.ns, 0, -1)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  local kinds = M.kinds(lines)
  -- A ticked item's indent while the lines under it (its details, sub-items,
  -- anything indented further) are drawn struck through with it.
  local ticked
  for i, line in ipairs(lines) do
    local row, k = i - 1, kinds[i]
    local text_from = 0
    local own = #line:match("^%s*")
    if ticked and (line:match("%S") == nil or own <= ticked) then ticked = nil end
    if ticked and M.options.strike_checked then hl(buf, row, own, #line, "NotesCheckedStrike", 120) end
    if k.kind == "check" and k.done and not ticked then ticked = k.indent end
    if k.kind == "fence" then
      hl(buf, row, 0, #line, "NotesFence")
    elseif k.kind == "code" then
      mark(buf, row, 0, { line_hl_group = "NotesMonoBlock", priority = 100 })
      hl(buf, row, 0, #line, "NotesMono")
    elseif k.kind == "title" or k.kind == "heading" or k.kind == "subheading" then
      local group = k.kind == "title" and "NotesTitle" or k.kind == "heading" and "NotesHeading" or "NotesSubheading"
      conceal(buf, row, 0, k.marker)
      mark(buf, row, 0, { virt_text = { { "▾ ", "NotesFold" } }, virt_text_pos = "inline", priority = 210 })
      hl(buf, row, k.marker, #line, group)
      text_from = k.marker
    elseif k.kind == "check" then
      local box = k.indent
      conceal(buf, row, box, box + k.marker, k.done and "☑" or "☐")
      mark(buf, row, box, { end_col = box + k.marker, hl_group = k.done and "NotesBoxDone" or "NotesBox", priority = 210 })
      if k.done then hl(buf, row, box + k.marker, #line, M.options.strike_checked and "NotesCheckedStrike" or "NotesChecked", 120) end
      text_from = box + k.marker
    elseif k.kind == "bullet" or k.kind == "dash" then
      conceal(buf, row, k.indent, k.indent + 1, k.kind == "bullet" and "•" or "–")
      hl(buf, row, k.indent, k.indent + 1, "NotesBullet", 210)
      text_from = k.indent + 2
    elseif k.kind == "number" then
      hl(buf, row, k.indent, k.indent + k.marker, "NotesBullet")
      text_from = k.indent + k.marker
    elseif k.kind == "quote" then
      local gt = line:find(">", 1, true) - 1
      conceal(buf, row, gt, gt + 1, "▎")
      hl(buf, row, gt, gt + 1, "NotesQuoteBar", 210)
      hl(buf, row, gt + 1, #line, "NotesQuote", 120)
      text_from = gt + 1
    elseif k.kind == "table" then
      require("notes.table").draw(buf, row, line, kinds[i - 1] == nil or kinds[i - 1].kind ~= "table", mark)
    end
    if k.kind ~= "code" and k.kind ~= "fence" then
      for _, span in ipairs(M.inline(line, text_from)) do
        local group = span.hl
        if span.note and not store.find_by_title(span.note) then group = "NotesNoteLinkMissing" end
        hl(buf, row, span.s, span.e, group, 160)
        for _, c in ipairs(span.conceal) do conceal(buf, row, c[1], c[2]) end
      end
    end
  end
end

-- What is at a buffer position: the inline span (a link, a note link, a tag)
-- covering it, if any.
function M.span_at(buf, row, col)
  local line = vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1] or ""
  for _, span in ipairs(M.inline(line)) do
    local from = span.conceal[1] and span.conceal[1][1] or span.s
    local to = span.conceal[#span.conceal] and span.conceal[#span.conceal][2] or span.e
    if col >= math.min(from, span.s) and col < math.max(to, span.e) then return span end
  end
  return nil
end

-- Redraws a note buffer whenever it changes, a moment after, so typing isn't
-- held up by it.
function M.attach(buf)
  local pending = false
  local function soon()
    if pending then return end
    pending = true
    vim.defer_fn(function()
      pending = false
      M.draw(buf)
    end, 20)
  end
  vim.api.nvim_create_autocmd({ "TextChanged", "TextChangedI", "InsertLeave", "BufWinEnter" }, { buffer = buf, callback = soon })
  M.draw(buf)
end

return M
