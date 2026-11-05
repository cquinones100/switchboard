-- Links: web addresses open in the browser, an image in the note
-- (notes.images) opens in Quick Look, [[Note title]] opens that note
-- (making it when there is none by that title), and a #tag shows the notes
-- that have it in the sidebar. Typing [[ offers every note's title to link to.

local M = {}

local render = require("notes.render")
local store = require("notes.store")

-- Follows what is at 0-based column `col` of `row`. Returns whether there
-- was something to follow.
function M.follow(buf, row, col)
  local span = render.span_at(buf, row - 1, col)
  if not span then return false end
  if span.image then
    local images = require("notes.images")
    local file = images.file(buf, span.image)
    if file then images.open(file) elseif span.image:match("^https?://") then vim.ui.open(span.image) else return false end
  elseif span.url then
    vim.ui.open(span.url)
  elseif span.note then
    local note = store.find_by_title(span.note)
    local path = note and note.path or store.create(require("notes.sidebar").state.folder or "", span.note)
    require("notes").open(path)
  elseif span.tag then
    require("notes.sidebar").show({ tag = span.tag })
  else
    return false
  end
  return true
end

-- In Insert mode, right after [[: the titles of every note, to pick from.
function M.complete()
  local col = vim.api.nvim_win_get_cursor(0)[2]
  local before = vim.api.nvim_get_current_line():sub(1, col)
  if before:sub(-2) ~= "[[" then return end
  local items = vim.tbl_map(function(title) return { word = title .. "]]", abbr = title, menu = "note" } end, store.titles())
  if #items > 0 then vim.fn.complete(col + 1, items) end
end

return M
