// Tests for the diff told as a story (nvim/lua/diffstory.lua and its use in
// nvim/lua/plugins/diffview.lua, M-d). Run with: node --test test/*.test.js
//
// The module's tests run Lua in a headless `nvim -u NONE`. The view's runs
// M-d's own Neovim (nvim/diff.lua) on a throwaway repository, with a stand-in
// claude and a temporary cache and HOME, so it reaches neither Claude nor your
// cache; it is skipped where lazy.nvim hasn't installed diffview, whose
// plugins it reads from your data directory.

const { test, after } = require("node:test");
const assert = require("node:assert/strict");
const { execFileSync, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const nvimDir = path.join(__dirname, "../nvim");
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "diff-story-")));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));

test("Haiku's answer becomes the story: every file once, one it leaves out in Other changes, generated files last, its diff cut short", () => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${nvimDir}`, "-l", "/dev/stdin"], {
    encoding: "utf8",
    input: `
      local ds = require("diffstory")
      local generated = function(p) return p:match("lock") ~= nil end
      local answer = [[Here: {"chapters": [
        {"title": "Add the field", "why": "The model gets a new field the rest reads.", "files": ["src/model.js", "src/model.test.js", "src/model.js", "nope.js"]},
        {"title": "Show it", "why": "The page shows it, a } and all.", "files": ["src/page.js", "pnpm-lock.yaml"]}
      ]}]]
      local story = ds.parse(answer, { "pnpm-lock.yaml", "src/model.js", "src/model.test.js", "src/page.js", "README.md" }, generated)
      local diffs = ds.split("diff --git a/src/a.js b/src/a.js\\n@@ -1 +1 @@\\n-x\\n+y\\ndiff --git a/old.js b/new.js\\nrename\\ndiff --cc src/both.js\\n@@@ -1,1 -1,1 +1,5 @@@\\n")
      -- A working tree's list: the unstaged changes, then the staged, a
      -- chapter in both, and a file the story doesn't know.
      local rows = {
        { chapter = 1, section = "working" }, { chapter = 2, section = "working" },
        { chapter = 1, section = "staged" }, { chapter = 1, section = "staged" }, { chapter = nil, section = "staged" }, { chapter = 2, section = "staged" },
      }
      local starts = ds.starts(rows)
      ds.LINES_PER_FILE = 2
      io.write(vim.json.encode({
        titles = vim.tbl_map(function(c) return c.title end, story.chapters),
        files = vim.tbl_map(function(c) return c.files end, story.chapters),
        place = story.place,
        heading = ds.heading(story, 2, 24),
        bad = ds.parse("no json here", { "a" }, generated) == nil,
        renamed = diffs["new.js"] ~= nil,
        conflicted = diffs["src/both.js"] and #diffs["src/both.js"] or 0,
        before_conflict = #diffs["new.js"],
        starts = starts,
        jumps = { ds.jump(starts, 0, 1), ds.jump(starts, 2, 1), ds.jump(starts, 4, 1), ds.jump(starts, 6, 1) or 0, ds.jump(starts, 4, -1), ds.jump(starts, 3, -1), ds.jump(starts, 1, -1) or 0 },
        input = ds.input({ { path = "src/a.js", additions = 1, deletions = 1 } }, diffs),
      }))
    `,
  });
  const result = JSON.parse(out);
  assert.deepEqual(result.titles, ["Add the field", "Show it", "Other changes", "Generated files"]);
  assert.deepEqual(result.files, [["src/model.js", "src/model.test.js"], ["src/page.js"], ["README.md"], ["pnpm-lock.yaml"]], "each once, unknown ones dropped, generated last");
  assert.deepEqual(result.place, { "src/model.js": 1, "src/model.test.js": 2, "src/page.js": 3, "README.md": 4, "pnpm-lock.yaml": 5 });
  assert.deepEqual(result.heading, [[["", "Normal"]], [["2/4 Show it", "Title"]], [["  The page shows it, a }", "Comment"]], [["  and all.", "Comment"]]], "its number and title, then its why, a brace in it kept");
  assert.equal(result.bad, true, "no story from an answer that isn't one");
  assert.equal(result.renamed, true, "a renamed file by its new path");
  assert.equal(result.conflicted, 3, "a conflicted file's combined diff by its path");
  assert.equal(result.before_conflict, 2, "and not under the file before it");
  assert.deepEqual(result.starts, [true, true, true, false, false, true], "a heading where a chapter starts in each section, a chapter in both headed in both");
  assert.deepEqual(result.jumps, [1, 3, 6, 0, 2, 2, 0], "]c and [c go between the headings as shown, across sections");
  assert.match(result.input, /^<diff>\n### src\/a\.js \(\+1 -1\)\ndiff --git a\/src\/a\.js b\/src\/a\.js\n@@ -1 \+1 @@\n… 2 more lines\n<\/diff>$/, "each file's diff cut short, inside one block");
});

const lazy = path.join(os.homedir(), ".local/share/nvim/lazy");
const ready = fs.existsSync(path.join(lazy, "diffview.nvim")) && fs.existsSync(path.join(lazy, "mini.base16"));

test("M-d's list is told as a story: in its chapters' order, each headed, the chapter in the bar, ]c going to the next", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const repo = path.join(tmp, "repo");
  const bin = path.join(tmp, "bin");
  const home = path.join(tmp, "home");
  [path.join(repo, "src"), path.join(repo, "test"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  // This repository's Neovim config as the config diff.lua reads its plugins' specs from.
  const config = path.join(tmp, "config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  fs.writeFileSync(path.join(repo, "src/model.js"), "a\n");
  fs.writeFileSync(path.join(repo, "src/ui.js"), "b\n");
  fs.writeFileSync(path.join(repo, "test/model.test.js"), "c\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  fs.writeFileSync(path.join(repo, "src/model.js"), "A\n");
  fs.writeFileSync(path.join(repo, "src/ui.js"), "B\n");
  fs.writeFileSync(path.join(repo, "test/model.test.js"), "C\n");
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "two"]);
  // A stand-in claude that answers with a story, and keeps what it was given.
  const given = path.join(tmp, "given.txt");
  const answer = JSON.stringify({ chapters: [{ title: "Change the model", why: "The data changes shape.", files: ["src/model.js", "test/model.test.js"] }, { title: "Show it", why: "The page shows it.", files: ["src/ui.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > '${given}'\nsleep 2\nprintf '%s\\n' '${answer}'\n`, { mode: 0o755 });
  // A headless Neovim run with -c writes io.write to stderr, so both are read.
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", "sleep 1000m", "-c", `lua
      local panel = require("diffview.lib").get_current_view().panel
      -- Drawn again every tenth of a second, as the list's own redraws may clear it.
      vim.wait(500, function()
        vim.g.telling = {}
        for _, m in ipairs(vim.api.nvim_buf_get_extmarks(panel.bufid, -1, 0, -1, { details = true })) do
          local text = m[4].virt_lines and m[4].virt_lines[1][1] and m[4].virt_lines[1][1][1] or ""
          if text:find("telling the story", 1, true) then vim.g.telling = vim.list_extend(vim.g.telling, { text }) end
        end
        return #vim.g.telling > 0
      end, 20)`, "-c", "sleep 4000m", "-c", `lua
      local view = require("diffview.lib").get_current_view()
      local panel = view.panel
      local marks = {}
      for _, m in ipairs(vim.api.nvim_buf_get_extmarks(panel.bufid, -1, 0, -1, { details = true })) do
        if m[4].virt_lines then table.insert(marks, { line = vim.api.nvim_buf_get_lines(panel.bufid, m[2], m[2] + 1, false)[1], title = m[4].virt_lines[#m[4].virt_lines > 2 and 2 or 1][1][1] }) end
      end
      local bars = {}
      for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do table.insert(bars, vim.wo[w].winbar) end
      vim.api.nvim_set_current_win(panel.winid)
      vim.api.nvim_win_set_cursor(0, { 3, 0 })
      vim.api.nvim_feedkeys(vim.keycode("]c"), "x", false)
      local jumped = vim.api.nvim_get_current_line()
      vim.api.nvim_feedkeys("E", "x", false)
      local wide = vim.api.nvim_win_get_width(panel.winid)
      vim.api.nvim_feedkeys("E", "x", false)
      local narrow = vim.api.nvim_win_get_width(panel.winid)
      local spinning = 0
      for _, m in ipairs(vim.api.nvim_buf_get_extmarks(panel.bufid, -1, 0, -1, { details = true })) do
        local text = m[4].virt_lines and m[4].virt_lines[1][1] and m[4].virt_lines[1][1][1] or ""
        if text:find("telling the story", 1, true) then spinning = spinning + 1 end
      end
      io.write(vim.json.encode({ telling = vim.g.telling, spinning = spinning, order = vim.tbl_map(function(e) return e.path end, panel:ordered_file_list()), marks = marks, bars = bars, jumped = jumped, wide = wide, narrow = narrow }))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
  );
  const result = JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.order, ["src/model.js", "test/model.test.js", "src/ui.js"], "the test beside its code, in the story's order");
  assert.deepEqual(result.marks.map((m) => [m.line.split(" ")[1], m.title]), [["model.js", "1/2 Change the model"], ["ui.js", "2/2 Show it"]], "each chapter headed over its first file");
  assert.ok(result.bars.some((bar) => bar.includes("1/2 Change the model · ")), "the chapter in the bar");
  assert.match(result.jumped, /ui\.js/, "]c goes to the next chapter");
  assert.deepEqual([result.wide, result.narrow], [32, 30], "E widens the list to its longest path, and again narrows it");
  assert.equal(result.telling.length, 1, "while Haiku reads the change, the list says so");
  assert.match(result.telling[0], /telling the story/);
  assert.equal(result.spinning, 0, "and stops saying so once the story is known");
  assert.match(fs.readFileSync(given, "utf8"), /<diff>[\s\S]*### src\/model\.js \(\+1 -1\)[\s\S]*\+A/, "Haiku read the diff itself");
});

test("in a working tree's list the headings and ]c follow its sections: a chapter in both headed in each, a file in both reached in the right one", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const repo = path.join(tmp, "worktree-repo");
  const bin = path.join(tmp, "worktree-bin");
  const home = path.join(tmp, "worktree-home");
  [path.join(repo, "src"), path.join(repo, "test"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "worktree-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  fs.writeFileSync(path.join(repo, "src/model.js"), "a\n");
  fs.writeFileSync(path.join(repo, "src/ui.js"), "b\n");
  fs.writeFileSync(path.join(repo, "test/model.test.js"), "c\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  // Staged: the model and its test. Unstaged: the model again, and the page.
  fs.writeFileSync(path.join(repo, "src/model.js"), "A\n");
  fs.writeFileSync(path.join(repo, "test/model.test.js"), "C\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  fs.writeFileSync(path.join(repo, "src/model.js"), "AA\n");
  fs.writeFileSync(path.join(repo, "src/ui.js"), "B\n");
  const answer = JSON.stringify({ chapters: [{ title: "Change the model", why: "The data changes shape.", files: ["src/model.js", "test/model.test.js"] }, { title: "Show it", why: "The page shows it.", files: ["src/ui.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\nprintf '%s\\n' '${answer}'\n`, { mode: 0o755 });
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen", "-c", "sleep 4000m", "-c", `lua
      local view = require("diffview.lib").get_current_view()
      local panel = view.panel
      local marks = {}
      for _, m in ipairs(vim.api.nvim_buf_get_extmarks(panel.bufid, -1, 0, -1, { details = true })) do
        if m[4].virt_lines then table.insert(marks, { row = m[2], title = m[4].virt_lines[#m[4].virt_lines > 2 and 2 or 1][1][1] }) end
      end
      local rows = {}
      for _, name in ipairs({ "working", "staged" }) do
        for _, comp in ipairs(panel.components[name].files.comp.components) do
          table.insert(rows, { section = name, path = comp.context.path, row = comp.lstart })
        end
      end
      local headed = {}
      for _, mark in ipairs(marks) do
        for _, r in ipairs(rows) do
          if r.row == mark.row then table.insert(headed, { r.section, r.path, mark.title }) end
        end
      end
      local function at() local item = panel:get_item_at_cursor() return { item.kind, item.path } end
      vim.api.nvim_set_current_win(panel.winid)
      local ui = vim.tbl_filter(function(r) return r.path == "src/ui.js" end, rows)[1]
      vim.api.nvim_win_set_cursor(0, { ui.row + 1, 0 })
      vim.api.nvim_feedkeys(vim.keycode("]c"), "x", false)
      local next = at()
      vim.api.nvim_feedkeys(vim.keycode("[c"), "x", false)
      local back = at()
      io.write(vim.json.encode({ headed = headed, next = next, back = back }))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "worktree-cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
  );
  const result = JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.headed, [
    ["working", "src/model.js", "1/2 Change the model"],
    ["working", "src/ui.js", "2/2 Show it"],
    ["staged", "src/model.js", "1/2 Change the model"],
  ], "each section headed where a chapter starts in it");
  assert.deepEqual(result.next, ["staged", "src/model.js"], "]c goes on into the next section, to the staged copy of a file in both");
  assert.deepEqual(result.back, ["working", "src/ui.js"], "[c comes back");
});
