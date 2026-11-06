-- Where notes live, and everything done to them on disk.
--
-- A note is a Markdown file. Its title is its first line, without the `# `
-- a title is written with, and its file is named after the title when it is
-- made, then renamed when the title changes (see rename_to_title), so the
-- folder reads like the notes in it. Folders are directories, nested as deep
-- as you like. The notes directory is $NOTES_DIR, or ~/Notes.
--
-- Two things in it aren't notes: `.trash/`, where a deleted note goes (the
-- "Recently Deleted" folder; deleting it from there removes it for good), and
-- `.notes.json`, which records the pinned notes and the note open last, by
-- their paths in the notes directory. Nothing else is kept anywhere, so the notes stay plain files any
-- editor can open.

local M = {}

local uv = vim.uv or vim.loop

M.TRASH = ".trash"
M.META = ".notes.json"
M.LOCK = ".notes.lock"

-- Its real path, so it matches the paths Neovim gives buffers: on macOS
-- /var and /tmp are links into /private, for one.
function M.root()
  local dir = vim.env.NOTES_DIR
  if not dir or dir == "" then dir = vim.fn.expand("~/Notes") end
  dir = vim.fs.normalize(dir)
  return vim.fs.normalize(uv.fs_realpath(dir) or dir)
end

function M.ensure()
  vim.fn.mkdir(M.root(), "p")
  vim.fn.mkdir(M.root() .. "/" .. M.TRASH, "p")
end

-- A path relative to the notes directory, for the metadata and the sidebar.
function M.relative(path)
  local root = M.root() .. "/"
  path = vim.fs.normalize(path)
  local parent = uv.fs_realpath(vim.fs.dirname(path))
  if parent then path = vim.fs.normalize(parent .. "/" .. vim.fs.basename(path)) end
  return path:sub(1, #root) == root and path:sub(#root + 1) or path
end

function M.absolute(rel)
  return M.root() .. "/" .. rel
end

local function read_file(path)
  local file = io.open(path, "r")
  if not file then return nil end
  local text = file:read("*a")
  file:close()
  return text
end

local function write_file(path, text)
  vim.fn.mkdir(vim.fs.dirname(path), "p")
  local file = assert(io.open(path, "w"))
  file:write(text)
  file:close()
end

-- ── Metadata ───────────────────────────────────────────────────────────────

function M.meta()
  local text = read_file(M.absolute(M.META))
  local ok, data = pcall(vim.json.decode, text or "")
  if not ok or type(data) ~= "table" then data = {} end
  data.pinned = type(data.pinned) == "table" and data.pinned or {}
  return data
end

-- Written to a file beside it, then moved into place, so a crash or a full
-- disk mid-write leaves the old pins rather than half a file.
function M.save_meta(data)
  local path = M.absolute(M.META)
  local temporary = string.format("%s.%d.tmp", path, uv.os_getpid())
  write_file(temporary, vim.json.encode(data))
  assert(uv.fs_rename(temporary, path))
end

-- ── One app at a time ──────────────────────────────────────────────────────

-- Notes save themselves as you type, so two apps open on the same notes
-- would write over each other. The one running holds .notes.lock, with its
-- process ID; another finding it held by a live process doesn't start. A
-- lock left by a process that has gone is taken over.
local function alive(pid)
  return pid and pid > 0 and uv.kill(pid, 0) == 0
end

-- Takes the lock, or returns nil and the process ID holding it.
function M.lock()
  M.ensure()
  local path = M.absolute(M.LOCK)
  local holder = tonumber(vim.trim(read_file(path) or ""))
  local me = uv.os_getpid()
  if holder and holder ~= me and alive(holder) then return nil, holder end
  write_file(path, tostring(me))
  return true
end

function M.unlock()
  local path = M.absolute(M.LOCK)
  if tonumber(vim.trim(read_file(path) or "")) == uv.os_getpid() then os.remove(path) end
end

function M.is_pinned(rel)
  return vim.tbl_contains(M.meta().pinned, rel)
end

function M.set_pinned(rel, on)
  local data = M.meta()
  data.pinned = vim.tbl_filter(function(p) return p ~= rel end, data.pinned)
  if on then table.insert(data.pinned, rel) end
  M.save_meta(data)
end

-- A note moved or renamed keeps its pin, and stays the note open last.
local function repin(from, to)
  local data = M.meta()
  for i, p in ipairs(data.pinned) do
    if p == from then data.pinned[i] = to end
  end
  if data.last == from then data.last = to end
  M.save_meta(data)
end

-- The note open last, which the app opens again when it starts: its path,
-- while it is still a note and not in Recently Deleted.
function M.set_last(rel)
  local data = M.meta()
  if data.last == rel then return end
  data.last = rel
  M.save_meta(data)
end

-- A note opened while Notes sits beside a tab (M-e), other than the one it
-- came with (@tab_note_arrived), becomes the tab's last note
-- (@tab_note_last) at once, so quitting Notes there keeps it, and the
-- marker is dropped, so going back to the one it came with counts too.
-- Never in Notes' own tab. In the background, and nothing outside tmux.
function M.opened_beside(rel)
  local pane = vim.env.TMUX_PANE or ""
  if (vim.env.TMUX or "") == "" or pane == "" then return end
  vim.system({ "tmux", "display", "-p", "-t", pane, "#{window_id}\t#{@notes}\t#{@tab_note_arrived}" }, { text = true }, function(out)
    local window, own, arrived = (out.stdout or ""):match("^([^\t]*)\t([^\t]*)\t([^\n]*)")
    if out.code ~= 0 or not window or own == "1" or arrived == rel then return end
    vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_note_last", rel, ";", "set-option", "-wu", "-t", window, "@tab_note_arrived" })
  end)
end

function M.last()
  local rel = M.meta().last
  if type(rel) ~= "string" or rel:sub(1, #M.TRASH + 1) == M.TRASH .. "/" then return nil end
  local path = M.absolute(rel)
  local stat = uv.fs_stat(path)
  return stat and stat.type == "file" and path or nil
end

-- ── Notes ──────────────────────────────────────────────────────────────────

-- The title from a note's text: its first line that isn't blank, without
-- Markdown's heading marks, or "New Note" for an empty one.
function M.title_of(text)
  for line in (text or ""):gmatch("[^\n]*") do
    local title = vim.trim((line:gsub("^%s*#+%s*", "")))
    if title ~= "" then return title end
  end
  return "New Note"
end

-- The text under the title, flattened to one line, for the sidebar.
function M.preview_of(text)
  local lines, seen_title = {}, false
  for line in (text or ""):gmatch("[^\n]*") do
    if not seen_title then
      if vim.trim(line) ~= "" then seen_title = true end
    else
      local plain = vim.trim((line:gsub("^%s*[#>*%-]+%s*", ""):gsub("%[[ xX]%]%s*", ""):gsub("[*_~=`|]", "")))
      if plain ~= "" then table.insert(lines, plain) end
      if #table.concat(lines, " ") > 80 then break end
    end
  end
  return table.concat(lines, " ")
end

-- A file name from a title: what a file name can hold, cut to a sensible
-- length, and never one already taken in that folder (another note's, or
-- `except`, the note's own when it is being renamed).
local function base_for(title)
  local base = title:gsub("[/\\:%c]", "-"):gsub("^%.+", ""):gsub("%s+", " ")
  base = vim.trim(base):sub(1, 80)
  return base == "" and "New Note" or base
end

function M.file_for(folder, title, except)
  local base = base_for(title)
  local dir = folder == "" and M.root() or M.absolute(folder)
  local candidate, n = dir .. "/" .. base .. ".md", 2
  while uv.fs_stat(candidate) and candidate ~= except do
    candidate = string.format("%s/%s %d.md", dir, base, n)
    n = n + 1
  end
  return candidate
end

local function note_entry(path)
  local stat = uv.fs_stat(path)
  if not stat or stat.type ~= "file" then return nil end
  local text = read_file(path) or ""
  local rel = M.relative(path)
  local folder = vim.fs.dirname(rel)
  return {
    path = path,
    rel = rel,
    folder = folder == "." and "" or folder,
    title = M.title_of(text),
    preview = M.preview_of(text),
    mtime = stat.mtime.sec,
    text = text,
  }
end

-- Every note in the notes directory, or in `folder` and the folders under
-- it, or in Recently Deleted when `folder` is M.TRASH. Newest first, pinned
-- notes first of all.
function M.notes(folder)
  local root = M.root()
  local start = (folder and folder ~= "") and M.absolute(folder) or root
  local in_trash = folder == M.TRASH
  local found = {}
  for name, type in vim.fs.dir(start, { depth = 20 }) do
    -- Hidden files and folders (Recently Deleted among them) aren't notes,
    -- except when Recently Deleted is what was asked for.
    local hidden = name:match("^%.") or name:match("/%.")
    if type == "file" and name:match("%.md$") and (in_trash or not hidden) then
      local entry = note_entry(start .. "/" .. name)
      if entry then table.insert(found, entry) end
    end
  end
  local pinned = M.meta().pinned
  for _, note in ipairs(found) do
    note.pinned = vim.tbl_contains(pinned, note.rel)
  end
  table.sort(found, function(a, b)
    if a.pinned ~= b.pinned then return a.pinned end
    if a.mtime ~= b.mtime then return a.mtime > b.mtime end
    return a.title:lower() < b.title:lower()
  end)
  return found
end

-- The folders, as paths relative to the notes directory, in tree order, each
-- with how deep it is and how many notes it holds directly.
function M.folders()
  local list = {}
  for name, type in vim.fs.dir(M.root(), { depth = 20 }) do
    if type == "directory" and not name:match("^%.") and not name:match("/%.") then
      table.insert(list, name)
    end
  end
  table.sort(list, function(a, b) return a:lower() < b:lower() end)
  local folders = {}
  for _, rel in ipairs(list) do
    local count = 0
    for name, type in vim.fs.dir(M.absolute(rel)) do
      if type == "file" and name:match("%.md$") then count = count + 1 end
    end
    local _, depth = rel:gsub("/", "")
    table.insert(folders, { rel = rel, name = vim.fs.basename(rel), depth = depth, count = count })
  end
  return folders
end

-- A new note in `folder` ("" for the notes directory itself), with `title`
-- as its first line, written as a title. Without one, the first line is an
-- empty title to type, and the file is "New Note" until there is one.
function M.create(folder, title)
  M.ensure()
  title = title and vim.trim(title) or ""
  local path = M.file_for(folder or "", title ~= "" and title or "New Note")
  write_file(path, "# " .. title .. "\n\n")
  return path
end

-- Renames a folder in place, to `name` in the same parent, keeping its
-- notes' pins and the note open last. A name has no slash, doesn't start with
-- a dot and isn't .., so a rename can't move a folder out of the notes
-- directory, into another folder, or out of sight. Returns its new path, or
-- nil and why not.
function M.rename_folder(from, name)
  name = vim.trim(name or "")
  if name == "" or name:match("^%.") or name:find("/", 1, true) or name:find("\\", 1, true) then
    return nil, "a folder's name can't be empty, start with a dot, or have a slash in it"
  end
  local parent = vim.fs.dirname(from)
  local to = (parent == "." and "" or parent .. "/") .. name
  if uv.fs_stat(M.absolute(to)) then return nil, to .. " is there already" end
  local ok, err = uv.fs_rename(M.absolute(from), M.absolute(to))
  if not ok then return nil, err end
  local data = M.meta()
  local function moved(p) return type(p) == "string" and p:sub(1, #from + 1) == from .. "/" and to .. p:sub(#from + 1) or p end
  data.pinned = vim.tbl_map(moved, data.pinned)
  data.last = moved(data.last)
  M.save_meta(data)
  M.relink(function(rel)
    local to_rel = moved(rel)
    return to_rel ~= rel and to_rel or nil
  end)
  return to
end

function M.create_folder(rel)
  rel = vim.trim(rel):gsub("^/+", ""):gsub("/+$", "")
  if rel == "" or rel:match("^%.") or rel:match("/%.") then return nil, "not a folder name" end
  vim.fn.mkdir(M.absolute(rel), "p")
  return rel
end

-- The tabs linked to a note (@tab_note, as /note-watch, M-e's question and
-- Notes' t set it, @tab_note_last, the one Notes last showed beside a
-- tab, and @tab_note_arrived, the one Notes came beside it with: paths in
-- the notes directory) follow it when it is renamed, moved or
-- squashed into another. `follow` gives a linked path's new one, or nil to
-- leave it. With `title` ({ from, to }, a note's title before and after),
-- a tab linked to that note and named after it, as a tab opened for a note
-- is, takes the new title as its name (its @tab_name, which the tab bar
-- then shows). One tmux call to look, and none
-- outside tmux.
function M.relink(follow, title)
  -- The note the sidebar daemon's mode works continuously
  -- (lib/sidebar/autopilot.js), kept in its state file, follows too.
  local state = (vim.env.TMUX_SIDEBAR_STATE_DIR or vim.fn.expand("~/.local/state/tmux-sidebar")) .. "/autopilot.json"
  local ok, mode = pcall(vim.json.decode, read_file(state) or "")
  local to_mode = ok and type(mode) == "table" and type(mode.note) == "string" and mode.note ~= "" and follow(mode.note)
  if to_mode then
    mode.note = to_mode
    if mode.said and mode.said:find("^Can't find the note") then mode.said = "" end
    local f = io.open(state .. ".tmp", "w")
    if f then
      f:write(vim.json.encode(mode))
      f:close()
      uv.fs_rename(state .. ".tmp", state)
    end
  end
  if (vim.env.TMUX or "") == "" then return end
  local listed = vim.system({ "tmux", "list-windows", "-a", "-F", "#{window_id}\t#{session_name}\t#{@tab_note}\t#{@tab_note_last}\t#{@tab_note_arrived}\t#{@tab_name}" }, { text = true }):wait()
  if listed.code ~= 0 then return end
  local renamed = {}
  for line in (listed.stdout or ""):gmatch("[^\n]+") do
    local window, session, note, last, arrived, name = unpack(vim.split(line, "\t", { plain = true }))
    local to_note = note ~= "" and follow(note) or nil
    local to_last = last ~= "" and follow(last) or nil
    local to_arrived = arrived ~= "" and follow(arrived) or nil
    if to_arrived then vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_note_arrived", to_arrived }):wait() end
    if to_note then vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_note", to_note }):wait() end
    if to_last then vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_note_last", to_last }):wait() end
    local linked = to_note or to_last or (title and (note == title.path or last == title.path))
    local named_for_it = title and linked and name and vim.trim(name) == vim.trim(title.from)
    if named_for_it and name ~= title.to then
      vim.system({ "tmux", "set-option", "-w", "-t", window, "@tab_name", title.to }):wait()
      renamed[session] = true
    end
  end
  for session in pairs(renamed) do
    pcall(vim.system, { vim.fn.expand("~/.config/tmux/window-names.sh"), session }, { detach = true })
  end
end

-- Moves a note's file, keeping its pin and the tabs linked to it, and returns
-- its new path. A note going to or coming back from Recently Deleted leaves
-- its tabs alone: a deleted note is no tab's note.
-- The images a note names (notes.images: ![name](.attachments/…), a path
-- from the note's folder) copied along when the note moves to another
-- folder, so its links still find them there; copied, not moved, since
-- another note (a squash's original, restored) may still name them where
-- they were. One already at the new place is left as it is.
local function carry_attachments(from, to)
  local old, new = vim.fs.dirname(from), vim.fs.dirname(to)
  if old == new then return end
  for rel in (read_file(to) or ""):gmatch("!%[[^%]]*%]%((%.attachments/[^%)%s]+)%)") do
    if not rel:find("..", 1, true) then
      local src, dst = old .. "/" .. rel, new .. "/" .. rel
      if uv.fs_stat(src) and not uv.fs_stat(dst) then
        vim.fn.mkdir(vim.fs.dirname(dst), "p")
        uv.fs_copyfile(src, dst)
      end
    end
  end
end

local function move_file(path, to)
  if path == to then return path end
  vim.fn.mkdir(vim.fs.dirname(to), "p")
  assert(uv.fs_rename(path, to))
  -- Into Recently Deleted and back too, so a deleted note still shows its
  -- images there, and one pasted there comes back with it; copied, so the
  -- notes still naming them where they were (a squash's merged note) keep them.
  carry_attachments(path, to)
  local from_rel, to_rel = M.relative(path), M.relative(to)
  repin(from_rel, to_rel)
  if not M.in_trash(path) and not M.in_trash(to) then
    M.relink(function(rel) return rel == from_rel and to_rel or nil end)
  end
  return to
end

-- A note whose title changed gets a file named for the new one. Returns its
-- path, which is the old one when the name already fits.
function M.rename_to_title(path)
  local text = read_file(path)
  if not text then return path end
  local folder = vim.fs.dirname(M.relative(path))
  folder = folder == "." and "" or folder
  local wanted = M.file_for(folder, M.title_of(text), path)
  local base = function(p) return vim.fs.basename(p):gsub(" %d+%.md$", ".md") end
  if base(wanted) == base(path) then return path end
  return move_file(path, wanted)
end

function M.move(path, folder)
  local text = read_file(path) or ""
  return move_file(path, M.file_for(folder, M.title_of(text)))
end

-- Deleting moves a note to Recently Deleted, keeping the folder it was in,
-- so restoring puts it back; deleting from there removes it for good.
function M.delete(path)
  local rel = M.relative(path)
  if M.meta().last == rel then M.set_last(nil) end
  if rel:sub(1, #M.TRASH + 1) == M.TRASH .. "/" then
    os.remove(path)
    M.set_pinned(rel, false)
    return nil
  end
  M.set_pinned(rel, false)
  local folder = vim.fs.dirname(rel)
  folder = folder == "." and M.TRASH or (M.TRASH .. "/" .. folder)
  return move_file(path, M.file_for(folder, M.title_of(read_file(path) or "")))
end

function M.restore(path)
  local rel = M.relative(path)
  local folder = vim.fs.dirname(rel):sub(#M.TRASH + 2)
  return move_file(path, M.file_for(folder, M.title_of(read_file(path) or "")))
end

function M.in_trash(path)
  return M.relative(path):sub(1, #M.TRASH + 1) == M.TRASH .. "/"
end

-- ── Search, tags and links ─────────────────────────────────────────────────

-- Notes whose title or text holds every word of `query`, ignoring case, each
-- with the first line that matches.
function M.search(query)
  local words = vim.split(query:lower(), "%s+", { trimempty = true })
  local results = {}
  for _, note in ipairs(M.notes()) do
    local hay = note.text:lower()
    local all = true
    for _, word in ipairs(words) do
      if not hay:find(word, 1, true) then all = false break end
    end
    if all and #words > 0 then
      for line in note.text:gmatch("[^\n]+") do
        if line:lower():find(words[1], 1, true) then
          note.match = vim.trim(line)
          break
        end
      end
      table.insert(results, note)
    end
  end
  return results
end

-- The #tags in a text: a # right after the start of a line or a space, then
-- letters, digits, - or _, not in code. Headings ("# Title") have a space
-- after the #, so they aren't tags.
function M.tags_in(text)
  local tags, seen, in_code = {}, {}, false
  for line in (text or ""):gmatch("[^\n]*") do
    if line:match("^%s*```") then
      in_code = not in_code
    elseif not in_code then
      local plain = line:gsub("`[^`]*`", "")
      for tag in (" " .. plain):gmatch("%s#([%w][%w_%-/]*)") do
        if not seen[tag:lower()] then
          seen[tag:lower()] = true
          table.insert(tags, tag)
        end
      end
    end
  end
  return tags
end

-- Every tag in every note, with how many notes have it, by name.
function M.tags()
  local counts, names = {}, {}
  for _, note in ipairs(M.notes()) do
    for _, tag in ipairs(M.tags_in(note.text)) do
      local key = tag:lower()
      if not counts[key] then
        counts[key] = 0
        names[key] = tag
      end
      counts[key] = counts[key] + 1
    end
  end
  local list = {}
  for key, count in pairs(counts) do
    table.insert(list, { name = names[key], count = count })
  end
  table.sort(list, function(a, b) return a.name:lower() < b.name:lower() end)
  return list
end

function M.tagged(tag)
  return vim.tbl_filter(function(note)
    for _, t in ipairs(M.tags_in(note.text)) do
      if t:lower() == tag:lower() then return true end
    end
    return false
  end, M.notes())
end

-- The note a [[link]] names, by its title, ignoring case: the newest when
-- several have that title.
function M.find_by_title(title)
  local wanted = vim.trim(title):lower()
  for _, note in ipairs(M.notes()) do
    if note.title:lower() == wanted then return note end
  end
  return nil
end

function M.titles()
  return vim.tbl_map(function(note) return note.title end, M.notes())
end

return M
