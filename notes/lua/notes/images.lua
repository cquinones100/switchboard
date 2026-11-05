-- Images in a note: one pasted from the clipboard is saved beside the note,
-- in .attachments/<the note's name>/ in its folder (a folder starting with a
-- dot, which the list leaves out), and written into the note as Markdown,
-- ![name](path), its path from the note's own folder, so Claude, reading the
-- note, can read the image too, and nothing about it leaves the Mac. Drawn in
-- the note under its line (M.draw) and opened in Quick Look by Enter or a
-- click (links.follow).

local M = {}

-- A name made only of letters, digits, dots and dashes, so the path in the
-- Markdown never holds a space or a bracket that would end it.
local function slug(text)
  local made = text:gsub("[^%w%.%-]+", "-"):gsub("%-+", "-"):gsub("^[%-%.]+", ""):gsub("%-+$", "")
  return made ~= "" and made or "note"
end

-- Saves the clipboard's image to `file` as a PNG, through macOS's own
-- pasteboard (AppKit, from JavaScript for Automation, which macOS has with
-- nothing to install): its PNG, or its TIFF made a PNG, as a screenshot
-- copied puts both there. AppleScript's `the clipboard as «class PNGf»`
-- can't read an image the screenshot tool hands over only when asked.
-- Returns whether there was an image to save. Tests replace it.
local GRAB = [[
ObjC.import("AppKit");
function run(argv) {
  const pb = $.NSPasteboard.generalPasteboard;
  let data = pb.dataForType("public.png");
  if (data.isNil()) {
    const tiff = pb.dataForType("public.tiff");
    if (tiff.isNil()) return "none";
    const rep = $.NSBitmapImageRep.imageRepWithData(tiff);
    if (rep.isNil()) return "none";
    data = rep.representationUsingTypeProperties($.NSBitmapImageFileTypePNG, $({}));
  }
  return data.writeToFileAtomically(argv[0], true) ? "ok" : "none";
}
]]
function M.grab(file)
  local result = vim.system({ "osascript", "-l", "JavaScript", "-e", GRAB, file }, { text = true }):wait()
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
-- data: one, or a path that leads out of the notes. Only what this gives is
-- drawn (M.draw), so the terminal is never handed a file outside the notes
-- or one a note's text made up.
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

-- ── Drawing ────────────────────────────────────────────────────────────────
-- A picture is drawn with the kitty graphics protocol's Unicode placeholders,
-- as M-p's PR image view draws its pages (tmux/pr-image.js): the terminal is
-- given the PNG's path once (it reads the file itself), as an image laid over
-- cells, and the note shows, under the image's line, virtual lines of a
-- placeholder character (U+10EEEE), each cell marked with its row and column
-- and coloured with the image's id. tmux places those like any other text,
-- so the picture sits exactly where its lines are, scrolls with them, and is
-- covered by a popup or a pane over them as text would be. It needs a
-- terminal that speaks kitty's protocol with placeholders (Ghostty, kitty)
-- and, inside tmux, the pane's allow-passthrough (bin/notes sets it). With the
-- notes_images setting off, an image is its name alone.

-- kitty's gen/rowcolumn-diacritics.txt: the nth mark means row or column n.
local MARKS = {
  0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F, 0x0346, 0x034A, 0x034B, 0x034C,
  0x0350, 0x0351, 0x0352, 0x0357, 0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
  0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484, 0x0485, 0x0486, 0x0487, 0x0592,
  0x0593, 0x0594, 0x0595, 0x0597, 0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
  0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611, 0x0612, 0x0613, 0x0614, 0x0615,
  0x0616, 0x0617, 0x0657, 0x0658, 0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
  0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2, 0x06E4, 0x06E7, 0x06E8, 0x06EB,
  0x06EC, 0x0730, 0x0732, 0x0733, 0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
  0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE, 0x07EF, 0x07F0, 0x07F1, 0x07F3,
  0x0816, 0x0817, 0x0818, 0x0819, 0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
  0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C, 0x082D, 0x0951, 0x0953, 0x0954,
  0x0F82, 0x0F83, 0x0F86, 0x0F87, 0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
  0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D, 0x1B6E, 0x1B6F, 0x1B70, 0x1B71,
  0x1B72, 0x1B73, 0x1CD0, 0x1CD1, 0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
  0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1, 0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5,
  0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9, 0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
  0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1, 0x20D4, 0x20D5, 0x20D6, 0x20D7,
  0x20DB, 0x20DC, 0x20E1, 0x20E7, 0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
  0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA, 0x2DEB, 0x2DEC, 0x2DED, 0x2DEE,
  0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2, 0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
  0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D, 0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1,
  0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5, 0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
  0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7, 0xAAB8, 0xAABE, 0xAABF, 0xAAC1,
  0xFE20, 0xFE21, 0xFE22, 0xFE23, 0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187,
  0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243, 0x1D244,
}

local PLACEHOLDER = vim.fn.nr2char(0x10EEEE)
M.ns = vim.api.nvim_create_namespace("notes.images")
-- The images given to the terminal, by id, with the size given, and the ids
-- each note on screen draws.
local sent = {}
local drawn = {}


-- A PNG's size in pixels, from its header, or nil.
local function png_size(file)
  local f = io.open(file, "rb")
  if not f then return nil end
  local head = f:read(24)
  f:close()
  if not head or #head < 24 or head:sub(1, 8) ~= "\137PNG\r\n\26\n" then return nil end
  local function u32(at) local a, b, c, d = head:byte(at, at + 3) return ((a * 256 + b) * 256 + c) * 256 + d end
  return u32(17), u32(21)
end

-- The terminal's cell in pixels: tmux knows it from the terminal
-- (client_cell_width and _height); else a common one.
local cell
local function cell_size()
  if cell then return cell end
  cell = { w = 8, h = 16 }
  if (vim.env.TMUX or "") ~= "" then
    local w, h = vim.fn.system({ "tmux", "display-message", "-p", "#{client_cell_width} #{client_cell_height}" }):match("^(%d+) (%d+)")
    if w and tonumber(w) > 0 and tonumber(h) > 0 then cell = { w = tonumber(w), h = tonumber(h) } end
  end
  return cell
end

-- An id for the image, 24 bits, never 0: what the placeholders' colour says.
local function id_of(file)
  local h = 2166136261
  for i = 1, #file do h = (bit.bxor(h, file:byte(i)) * 16777619) % 4294967296 end
  local id = h % 16777215
  return id == 0 and 1 or id
end

-- Sends the terminal a kitty graphics command, through tmux when inside it:
-- through Neovim's UI (nvim_ui_send, Neovim 0.12 on), else written to the
-- pane's terminal itself.
local function send(command)
  local seq = "\27_G" .. command .. "\27\\"
  if (vim.env.TMUX or "") ~= "" then seq = "\27Ptmux;" .. seq:gsub("\27", "\27\27") .. "\27\\" end
  if vim.api.nvim_ui_send then return pcall(vim.api.nvim_ui_send, seq) end
  local tty = "/dev/tty"
  if (vim.env.TMUX_PANE or "") ~= "" then
    local pane = vim.trim(vim.fn.system({ "tmux", "display-message", "-p", "-t", vim.env.TMUX_PANE, "#{pane_tty}" }))
    if pane ~= "" then tty = pane end
  end
  local out = io.open(tty, "w")
  if out then
    out:write(seq)
    out:close()
  end
end

-- Takes back from the terminal the pictures `buf` drew that are not in
-- `keep` and no other note on screen draws, so the terminal keeps only those
-- shown (Ghostty holds at most 320 MB of them), and one shown again is sent
-- again.
local function release(buf, keep)
  for id in pairs(drawn[buf] or {}) do
    local elsewhere = false
    for other, ids in pairs(drawn) do
      if other ~= buf and ids[id] then elsewhere = true end
    end
    if not (keep and keep[id]) and not elsewhere and sent[id] then
      send(("a=d,d=I,i=%d,q=2"):format(id))
      sent[id] = nil
    end
  end
  drawn[buf] = keep
end

-- A picture the terminal can't take as it is (it reads PNG alone): made a
-- PNG in Neovim's cache with macOS's sips, once for each version of the
-- file. Returns that PNG, or nil.
local KINDS = { jpg = true, jpeg = true, gif = true, webp = true, heic = true, tif = true, tiff = true, bmp = true }
local function as_png(file)
  local ext = (file:match("%.(%w+)$") or ""):lower()
  if not KINDS[ext] then return nil end
  local stat = vim.uv.fs_stat(file)
  if not stat then return nil end
  local dir = vim.fn.stdpath("cache") .. "/notes-images"
  local out = ("%s/%s.png"):format(dir, vim.fn.sha256(file .. ":" .. stat.mtime.sec .. ":" .. stat.size))
  if not vim.uv.fs_stat(out) then
    vim.fn.mkdir(dir, "p")
    local made = vim.system({ "sips", "-s", "format", "png", file, "--out", out }, { text = true }):wait()
    if made.code ~= 0 then
      os.remove(out)
      return nil
    end
  end
  return out
end

-- Whether the terminal speaks kitty's graphics protocol with placeholders:
-- Ghostty or kitty, by the terminal tmux's client is (its termname), or
-- outside tmux by $TERM and $TERM_PROGRAM. Elsewhere an image is its name,
-- with no placeholder lines to show as odd characters.
local capable
local function terminal_can()
  if capable ~= nil then return capable end
  local name = (vim.env.TERM or "") .. " " .. (vim.env.TERM_PROGRAM or "")
  if (vim.env.TMUX or "") ~= "" then name = vim.fn.system({ "tmux", "display-message", "-p", "#{client_termname} #{client_termtype}" }) end
  capable = name:lower():find("ghostty", 1, true) ~= nil or name:lower():find("kitty", 1, true) ~= nil
  return capable
end

local function on()
  if not terminal_can() then return false end
  if (vim.env.TMUX or "") == "" then return true end
  return vim.trim(vim.fn.system({ "tmux", "show", "-gqv", "@setting_notes_images" })) ~= "off"
end

-- Draws each image `buf` names under its line, as wide as the window lets
-- it (at most its own width) and at most two fifths of the window's height.
-- Called by render.draw after the rest of a note is drawn.
function M.draw(buf, lines)
  vim.api.nvim_buf_clear_namespace(buf, M.ns, 0, -1)
  local win = vim.fn.bufwinid(buf)
  if win == -1 or not on() then return release(buf, nil) end
  local keep = {}
  local info = vim.fn.getwininfo(win)[1]
  -- Only the pictures on screen, or a screen's height from it, are given to
  -- the terminal (and taken back once scrolled away), so a note of many
  -- screenshots never fills its image store; the rest keep their lines.
  local near = { info.topline - info.height, info.botline + info.height }
  local size = cell_size()
  local render = require("notes.render")
  local kinds = render.kinds(lines)
  for i, line in ipairs(lines) do
    -- Not in a code block, which shows its text as it is.
    local plain = kinds[i] and (kinds[i].kind == "code" or kinds[i].kind == "fence")
    -- Every image the line names, one under the other.
    local virt = {}
    for _, span in ipairs(plain and {} or render.inline(line)) do
      local file = span.image and M.resolve(vim.api.nvim_buf_get_name(buf), span.image)
      local pw, ph
      if file then
        pw, ph = png_size(file)
        if not pw then
          file = as_png(file)
          if file then pw, ph = png_size(file) end
        end
      end
      if pw and ph and pw > 0 and ph > 0 then
        local indent = #line:match("^%s*")
        local room = math.min(info.width - info.textoff - indent - 1, #MARKS)
        local cols = math.max(1, math.min(room, math.ceil(pw / size.w)))
        local rows = math.max(1, math.ceil(cols * size.w * ph / pw / size.h))
        local most = math.max(1, math.min(math.floor(info.height * 0.4), #MARKS))
        if rows > most then
          rows = most
          cols = math.max(1, math.min(room, math.floor(rows * size.h * pw / ph / size.w)))
        end
        -- An id for this picture at this size: the same image drawn twice at
        -- two sizes (under an item and not) is two images to the terminal.
        -- And a file changed since is a new image, not the one sent before.
        local stat = vim.uv.fs_stat(file)
        local id = id_of(("%s:%dx%d:%s:%s"):format(file, cols, rows, stat and stat.mtime.sec or 0, stat and stat.size or 0))
        local shown = i >= near[1] and i <= near[2]
        if shown then keep[id] = true end
        if shown and sent[id] ~= cols .. "x" .. rows then
          send(("a=T,U=1,f=100,t=f,q=2,i=%d,c=%d,r=%d;%s"):format(id, cols, rows, vim.base64.encode(file)))
          sent[id] = cols .. "x" .. rows
        end
        local group = "NotesImage" .. id
        vim.api.nvim_set_hl(0, group, { fg = id })
        for r = 1, rows do
          local cells = {}
          for c = 1, cols do cells[c] = PLACEHOLDER .. vim.fn.nr2char(MARKS[r]) .. vim.fn.nr2char(MARKS[c]) end
          table.insert(virt, { { string.rep(" ", indent) }, { table.concat(cells), group } })
        end
      end
    end
    if #virt > 0 then pcall(vim.api.nvim_buf_set_extmark, buf, M.ns, i - 1, 0, { virt_lines = virt }) end
  end
  release(buf, keep)
end

-- The terminal may have lost them (a new terminal attached to tmux, say):
-- sent again when Neovim comes back into focus or is resumed, and on a
-- resize, which can change the cell size too.
-- Every note on screen drawn again, its pictures with it.
local function redraw()
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    local buf = vim.api.nvim_win_get_buf(win)
    if vim.bo[buf].filetype == "notes" then require("notes.render").draw(buf) end
  end
end
vim.api.nvim_create_autocmd({ "FocusGained", "VimResume", "VimResized" }, {
  callback = function()
    sent, cell, capable = {}, nil, nil
    vim.schedule(redraw)
  end,
})
-- A colorscheme loaded clears the highlights whose colour is each picture's
-- id (the theme switching between dark and light reloads it).
vim.api.nvim_create_autocmd("ColorScheme", { callback = function() vim.schedule(redraw) end })
-- A note no longer on screen: its pictures taken back from the terminal.
vim.api.nvim_create_autocmd({ "BufWinLeave", "BufDelete" }, {
  callback = function(ev)
    if drawn[ev.buf] then release(ev.buf, nil) end
  end,
})
-- Scrolled: the pictures come near the screen given to the terminal, and
-- those gone far from it taken back.
vim.api.nvim_create_autocmd("WinScrolled", {
  callback = function()
    vim.schedule(function()
      for _, win in ipairs(vim.api.nvim_list_wins()) do
        local buf = vim.api.nvim_win_get_buf(win)
        if vim.bo[buf].filetype == "notes" then M.draw(buf, vim.api.nvim_buf_get_lines(buf, 0, -1, false)) end
      end
    end)
  end,
})
-- A window narrower or wider (the sidebar put away or shown, Notes moved
-- beside a tab): each picture sized to it again.
vim.api.nvim_create_autocmd("WinResized", { callback = function() vim.schedule(redraw) end })

return M
