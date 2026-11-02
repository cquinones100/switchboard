-- Reference-style links: a link written [text](https://…) becomes
-- [text][1] in its line, its address gathered at the end of the note as
-- "[1]: https://…". Notes hides a link's address, but Neovim wraps a line by
-- its text as written, hidden characters included, so a long address in the
-- middle of a line broke it early, after a gap. In a line, only [1] is left.
--
-- Done on save, to the lines of a note that changed or not alike, except the
-- one being typed in, which waits for the next save. An address already
-- listed keeps its number. Inside `code` and code blocks, nothing changes.
-- The notes_reference_links setting turns it off.

local M = {}

-- A definition, "[1]: https://…", a title after it ("Old") allowed.
M.DEF = "^%[(%d+)%]:%s*(%S+)"

-- The definitions in `lines`: number -> address.
function M.defs(lines)
  local defs = {}
  local fence = false
  for _, line in ipairs(lines) do
    if line:match("^%s*```") then fence = not fence end
    local n, url = line:match(M.DEF)
    if n and not fence then defs[tonumber(n)] = url end
  end
  return defs
end

-- Whether the setting is on: tmux's copy of it, as the app's other settings
-- are read, on unless it says off, and on outside tmux.
-- Read again at most every 30 seconds, so a change in Settings reaches an
-- open app without a process started on every save.
local on, read_at = nil, 0
function M.enabled()
  if on == nil or vim.uv.now() - read_at > 30000 then
    read_at = vim.uv.now()
    on = not (vim.env.TMUX and vim.trim(vim.fn.system({ "tmux", "show", "-gqv", "@setting_notes_reference_links" })) == "off")
  end
  return on
end

-- Converts buf's inline links, leaving row `skip` (0-based) alone. Returns
-- whether it changed anything.
function M.convert(buf, skip)
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  -- A code block left open (one being typed) would hold the definitions
  -- added at the end: nothing changes until it is closed.
  local open = false
  for _, line in ipairs(lines) do
    if line:match("^%s*```") then open = not open end
  end
  if open then return false end
  local defs = M.defs(lines)
  local by_url, max = {}, 0
  for n, url in pairs(defs) do
    if not by_url[url] or n < by_url[url] then by_url[url] = n end
    if n > max then max = n end
  end
  local added = {}
  local fence = false
  for i, line in ipairs(lines) do
    local row = i - 1
    if line:match("^%s*```") then
      fence = not fence
    elseif not fence and row ~= skip and not line:match(M.DEF) then
      -- `code` blanked, so a link written inside one is left as it is.
      local masked = line:gsub("`[^`]+`", function(code) return string.rep("\1", #code) end)
      local found = {}
      local init = 1
      while true do
        local s, e, text, url = masked:find("%[([^%]]+)%]%((https?://[^%)%s]+)%)", init)
        if not s then break end
        -- An address with a ( in it ends at a ) of its own the pattern
        -- can't tell from the link's: such a link is left as written.
        if not url:find("%(") and masked:sub(e + 1, e + 1) ~= ")" then table.insert(found, { s = s, e = e, text = text, url = url }) end
        init = e + 1
      end
      -- Numbered left to right, so they read in order top to bottom, then
      -- replaced from the right, so each one's columns still hold.
      for _, link in ipairs(found) do
        link.n = by_url[link.url]
        if not link.n then
          max = max + 1
          link.n = max
          by_url[link.url] = max
          table.insert(added, ("[%d]: %s"):format(max, link.url))
        end
      end
      for k = #found, 1, -1 do
        local link = found[k]
        local open = link.s + #link.text + 1 -- the ( after the ]
        vim.api.nvim_buf_set_text(buf, row, open, row, link.e, { ("[%d]"):format(link.n) })
      end
    end
  end
  if #added == 0 then return false end
  -- Sorted by number, after a blank line unless the note already ends in its
  -- definitions.
  table.sort(added, function(a, b) return tonumber(a:match("%d+")) < tonumber(b:match("%d+")) end)
  local count = vim.api.nvim_buf_line_count(buf)
  local last = vim.api.nvim_buf_get_lines(buf, count - 1, count, false)[1] or ""
  if last == "" and count > 1 then
    local before = vim.api.nvim_buf_get_lines(buf, count - 2, count - 1, false)[1] or ""
    if before:match(M.DEF) then count = count - 1 end
  elseif not last:match(M.DEF) then
    table.insert(added, 1, "")
  end
  vim.api.nvim_buf_set_lines(buf, count, count, false, added)
  return true
end

-- Text copied out of a note (a yank or a delete) carries its links whole:
-- each [text][1] in it written back as [text](https://…), from the note's
-- definitions, so pasted into another note it still goes where it went, and
-- that note's next save numbers it there.
function M.expand(lines, defs)
  return vim.tbl_map(function(line)
    return (line:gsub("%[([^%]]+)%]%[(%d+)%]", function(text, n)
      local url = defs[tonumber(n)]
      return url and ("[%s](%s)"):format(text, url) or nil
    end))
  end, lines)
end

-- On TextYankPost in a note: the register just written, expanded.
function M.on_yank(buf)
  local event = vim.v.event
  local name = event.regname ~= "" and event.regname or '"'
  local defs = M.defs(vim.api.nvim_buf_get_lines(buf, 0, -1, false))
  if next(defs) == nil then return end
  -- Its definition lines go: each link now carries its address, and a
  -- copied [1]: would claim the number of a link in the note it lands in.
  local kept = vim.tbl_filter(function(line) return not line:match(M.DEF) end, event.regcontents)
  if #kept == 0 then return end
  local expanded = M.expand(kept, defs)
  if vim.deep_equal(expanded, event.regcontents) then return end
  vim.fn.setreg(name, expanded, event.regtype)
  if name == '"' and event.operator == "y" then vim.fn.setreg("0", expanded, event.regtype) end
end

return M
