-- A note's stable ID, which survives the note being renamed or moved, in the
-- app or outside it: a last line `<!-- note-id: n-<12 hex> -->`, the same line
-- lib/note-id.js reads and writes for wtn's claims and the work-a-note mode.
-- The app gives a note one when it makes it or first opens it, keeps it last
-- whenever it saves (putting it back when typing removed it), hides it
-- (render.lua), and gives a copy of a note, which carries the original's, an
-- ID of its own.

local M = {}

local uv = vim.uv or vim.loop

M.PATTERN = "^%s*<!%-%- note%-id: (n%-%x+) %-%->%s*$"

function M.new()
  local bytes = uv.random(6)
  return "n-" .. (bytes:gsub(".", function(c) return ("%02x"):format(c:byte()) end))
end

function M.line(id)
  return ("<!-- note-id: %s -->"):format(id)
end

-- The ID a line holds, or nil.
function M.of_line(line)
  local id = (line or ""):match(M.PATTERN)
  return id and #id == 14 and id or nil
end

-- The ID in a list of lines, from the last line holding one, and its row.
function M.in_lines(lines)
  for i = #lines, 1, -1 do
    local id = M.of_line(lines[i])
    if id then return id, i end
  end
  return nil
end

function M.in_text(text)
  return M.in_lines(vim.split(text or "", "\n", { plain = true }))
end

-- The text without its ID line, for what shows or sends a note's words.
function M.strip(text)
  local kept = {}
  for _, line in ipairs(vim.split(text or "", "\n", { plain = true })) do
    if not M.of_line(line) then table.insert(kept, line) end
  end
  return table.concat(kept, "\n")
end

-- The text with `id` as its last line, after a blank one, and no other ID
-- line.
function M.with(text, id)
  local kept = M.strip(text):gsub("\n*$", "")
  return (kept == "" and "" or kept .. "\n\n") .. M.line(id) .. "\n"
end

-- A note's lines with `id` as the last, after a blank line, and no other ID
-- line, leaving every other line as it is; nil when they are so already.
-- An ID line formatting wrapped in other text (a checklist box, bold) loses
-- the ID, and the line goes when nothing else is left on it.
local EMBEDDED = "<!%-%- note%-id: n%-%x+ %-%->"
function M.kept_last(lines, id)
  local out = {}
  for _, line in ipairs(lines) do
    if not M.of_line(line) then
      if line:find(EMBEDDED) then
        line = line:gsub(EMBEDDED, "")
        if line:find("%w") then table.insert(out, line) end
      else
        table.insert(out, line)
      end
    end
  end
  if #out > 0 and out[#out] ~= "" then table.insert(out, "") end
  table.insert(out, M.line(id))
  return not vim.deep_equal(out, lines) and out or nil
end

-- The ID of the note at `path`, read from its last 300 bytes, or from the
-- whole note when lines were added below its ID since.
function M.of_file(path)
  local file = io.open(path, "r")
  if not file then return nil end
  local size = file:seek("end")
  file:seek("set", math.max(0, size - 300))
  local id = M.in_text(file:read("*a"))
  if not id and size > 300 then
    file:seek("set", 0)
    id = M.in_text(file:read("*a"))
  end
  file:close()
  return id
end

-- The other notes holding `id`, besides `path`, each with its modified time.
function M.others(path, id)
  local store = require("notes.store")
  local root = store.root()
  local found = {}
  for name, kind in vim.fs.dir(root, { depth = 20 }) do
    if kind == "file" and name:match("%.md$") and not name:match("^%.") and not name:match("/%.") then
      local other = root .. "/" .. name
      if vim.fs.normalize(other) ~= vim.fs.normalize(path) and M.of_file(other) == id then
        local stat = uv.fs_stat(other)
        table.insert(found, { path = other, mtime = stat and stat.mtime.sec + stat.mtime.nsec / 1e9 or 0 })
      end
    end
  end
  return found
end

-- Whether the note at `path`, holding `id`, is a copy of another note holding
-- the same: the one changed later, or, changed at the same moment, the one
-- whose path sorts later, is the copy, as lib/note-id.js decides.
function M.is_copy(path, id)
  local stat = uv.fs_stat(path)
  local mtime = stat and stat.mtime.sec + stat.mtime.nsec / 1e9 or 0
  for _, other in ipairs(M.others(path, id)) do
    if other.mtime < mtime or (other.mtime == mtime and other.path < path) then return true, other.path end
  end
  return false
end

return M
