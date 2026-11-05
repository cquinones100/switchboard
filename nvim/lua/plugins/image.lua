-- Images in a note drawn in the note itself, under the line that names them
-- (![name](path), notes/lua/notes/images.lua), in the notes app only: drawn
-- through the terminal's graphics protocol (Ghostty speaks kitty's), which
-- tmux passes on for the notes pane alone (tmux/notes-open.sh sets that
-- pane's allow-passthrough). Needs ImageMagick's magick, which ./setup.sh
-- offers among the optional parts; without it, or with the notes_images
-- setting off, an image is its name, which Enter opens in Quick Look.
local function wanted()
  if not vim.g.notes_app or vim.fn.executable("magick") == 0 then return false end
  if not vim.env.TMUX then return true end
  return vim.trim(vim.fn.system({ "tmux", "show", "-gqv", "@setting_notes_images" })) ~= "off"
end

return {
  "3rd/image.nvim",
  version = "v1.5.1",
  -- Its rockspec asks for the magick Lua rock, which processor = "magick_cli"
  -- below doesn't use: not built, so no Lua toolchain (hererocks) is fetched.
  build = false,
  cond = wanted,
  ft = "notes",
  opts = {
    backend = "kitty",
    processor = "magick_cli",
    integrations = {
      markdown = {
        enabled = true,
        filetypes = { "notes" },
        clear_in_insert_mode = false,
        only_render_image_at_cursor = false,
        -- Only the note's own files: an image on the web is never fetched.
        download_remote_images = false,
        -- Only an image file inside the notes (notes.images.resolve), never a
        -- data: address or a path leading out of them, and only a picture's
        -- kind of file, since ImageMagick reads whatever it is handed.
        resolve_image_path = function(note, path)
          local ok, images = pcall(require, "notes.images")
          local file = ok and images.resolve(note, path)
          if not file then return nil end
          local ext = file:match("%.(%w+)$")
          return ext and vim.tbl_contains({ "png", "jpg", "jpeg", "gif", "webp" }, ext:lower()) and file or nil
        end,
      },
      neorg = { enabled = false },
      typst = { enabled = false },
      html = { enabled = false },
      css = { enabled = false },
    },
    max_width_window_percentage = 100,
    max_height_window_percentage = 40,
    window_overlap_clear_enabled = true,
    tmux_show_only_in_active_window = true,
  },
  config = function(_, opts)
    -- image.nvim keeps its pictures only while the tmux window it started in
    -- is the one in front, and Notes moves between windows (M-e beside a
    -- tab, back to its own): the window asked about is made the one the
    -- notes pane is in now, so the pictures stay while that window is in
    -- front, and go when another is.
    local tmux = require("image/utils/tmux")
    local front = tmux.get_window_id
    tmux.get_window_id = function()
      local shown = front()
      local pane = vim.env.TMUX_PANE
      if not (shown and pane) then return shown end
      local mine = vim.trim(vim.fn.system({ "tmux", "display-message", "-p", "-t", pane, "#{window_id}" }))
      return mine == shown and "notes" or shown
    end
    require("image").setup(opts)
  end,
}
