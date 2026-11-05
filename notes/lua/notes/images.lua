-- Images in a note: one pasted from the clipboard is saved beside the note,
-- in .attachments/<the note's name>/ in its folder (a folder starting with a
-- dot, which the list leaves out), and written into the note as Markdown,
-- ![name](path), its path from the note's own folder, so Claude, reading the
-- note, can read the image too, and nothing about it leaves the Mac. Drawn in
-- the note by image.nvim where it is set up (nvim/lua/plugins/image.lua), and
-- opened in Quick Look by Enter or a click (links.follow) everywhere.

local M = {}

-- A name made only of letters, digits, dots and dashes, so the path in the
-- Markdown never holds a space or a bracket that would end it.
local function slug(text)
  local made = text:gsub("[^%w%.%-]+", "-"):gsub("%-+", "-"):gsub("^[%-%.]+", ""):gsub("%-+$", "")
  return made ~= "" and made or "note"
end

-- Saves the clipboard's image to `file` as a PNG, through AppleScript, which
-- macOS has with no tool of its own to install. Returns whether there was an
-- image to save. Tests replace it.
function M.grab(file)
  local script = {
    "set f to POSIX file " .. vim.fn.json_encode(file),
    "try",
    "  set png to the clipboard as «class PNGf»",
    "on error",
    "  return \"none\"",
    "end try",
    "set out to open for access f with write permission",
    "write png to out",
    "close access out",
    "return \"ok\"",
  }
  local args = { "osascript" }
  for _, line in ipairs(script) do vim.list_extend(args, { "-e", line }) end
  local result = vim.system(args, { text = true }):wait()
  return result.code == 0 and vim.trim(result.stdout or "") == "ok" and vim.uv.fs_stat(file) ~= nil
end

-- Pastes the clipboard's image into `buf` on a line of its own below the
-- cursor's, indented under it when that is a list or checklist item, so the
-- image goes with the item (ticked, moved) and is read as part of it.
-- Returns the path written, or nil and why not.
function M.paste(buf)
  local note = vim.api.nvim_buf_get_name(buf)
  if note == "" then return nil, "not a note" end
  local folder = vim.fs.dirname(note)
  local name = slug(vim.fn.fnamemodify(note, ":t:r"))
  local dir = folder .. "/.attachments/" .. name
  vim.fn.mkdir(dir, "p")
  local stamp = os.date("%Y-%m-%d-%H%M%S")
  local file, n = dir .. "/image-" .. stamp .. ".png", 1
  while vim.uv.fs_stat(file) do
    n = n + 1
    file = ("%s/image-%s-%d.png"):format(dir, stamp, n)
  end
  if not M.grab(file) then
    pcall(vim.uv.fs_rmdir, dir)
    return nil, "no image on the clipboard"
  end
  local rel = ".attachments/" .. name .. "/" .. vim.fn.fnamemodify(file, ":t")
  local row = vim.api.nvim_win_get_cursor(0)[1]
  local line = vim.api.nvim_buf_get_lines(buf, row - 1, row, false)[1] or ""
  local indent = line:match("^%s*")
  if line:match("^%s*[-*+] ") or line:match("^%s*%d+[.)] ") then indent = indent .. "  " end
  vim.api.nvim_buf_set_lines(buf, row, row, false, { ("%s![%s](%s)"):format(indent, vim.fn.fnamemodify(file, ":t:r"), rel) })
  return rel
end

-- An image's path in a note made a file on disk, from the note's folder, or
-- nil for one outside the notes directory or not there: a web address, a
-- data: one, or a path that leads out of the notes. image.nvim draws only
-- what this gives (nvim/lua/plugins/image.lua), so it never reads a file
-- outside the notes or one a note's text made up.
function M.resolve(note, path)
  if path:match("^%a[%w+.-]*:") then return nil end
  -- Real paths, so neither a .. nor a link leads outside the notes.
  local full = vim.uv.fs_realpath(vim.fs.dirname(note) .. "/" .. path)
  local root = vim.uv.fs_realpath(require("notes.store").root())
  if not (full and root) or full:sub(1, #root + 1) ~= root .. "/" then return nil end
  return full
end

function M.file(buf, path)
  return M.resolve(vim.api.nvim_buf_get_name(buf), path)
end

-- Opens it in Quick Look, without leaving the terminal for an app.
function M.open(file)
  vim.system({ "qlmanage", "-p", file }, { stdout = false, stderr = false })
end

return M
