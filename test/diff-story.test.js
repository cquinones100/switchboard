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
const { execFileSync, spawn, spawnSync } = require("node:child_process");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");

const nvimDir = path.join(__dirname, "../nvim");
const tmp = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "diff-story-")));
after(() => fs.rmSync(tmp, { recursive: true, force: true }));
const lazy = path.join(os.homedir(), ".local/share/nvim/lazy");
const ready = fs.existsSync(path.join(lazy, "diffview.nvim")) && fs.existsSync(path.join(lazy, "mini.base16"));

test("Haiku's answer becomes the story: every file once, one it leaves out in Other changes, generated files last, its diff read whole", () => {
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
      local diffs, hunks = ds.split("diff --git a/src/a.js b/src/a.js\\n@@ -1 +1 @@\\n-x\\n+y\\ndiff --git a/old.js b/new.js\\nrename\\ndiff --cc src/both.js\\n@@@ -1,1 -1,1 +1,5 @@@\\n")
      -- A working tree's list: the unstaged changes, then the staged, a
      -- chapter in both, and a file the story doesn't know.
      local rows = {
        { chapter = 1, section = "working" }, { chapter = 2, section = "working" },
        { chapter = 1, section = "staged" }, { chapter = 1, section = "staged" }, { chapter = nil, section = "staged" }, { chapter = 2, section = "staged" },
      }
      local starts = ds.starts(rows)
      -- An answer cut short mid-way through its third chapter's title, the
      -- first's why quoting "title" and its title an escaped quote.
      local cut = [[{"chapters": [{"title": "Add the \\"pick\\" column", "why": "Says \\"title\\": \\"no\\".", "files": ["a"]}, {"title": "Default it on", "why": "b", "files": ["c"]}, {"title": "Sho]]
      local partial = ds.partial(cut)
      local progress = ds.progress({ text = cut, started = 100 }, { files = 3, additions = 12, deletions = 4 }, "*", 175)
      io.write(vim.json.encode({
        titles = vim.tbl_map(function(c) return c.title end, story.chapters),
        files = vim.tbl_map(function(c) return c.files end, story.chapters),
        place = story.place,
        heading = ds.heading(story, 2, 24),
        narrow = ds.heading({ chapters = { { title = "Check the one-worktree cleanup mode", why = "The --worktree path must keep the same safety rules everywhere." } } }, 1, 16),
        bad = ds.parse("no json here", { "a" }, generated) == nil,
        renamed = diffs["new.js"] ~= nil,
        conflicted = diffs["src/both.js"] and #diffs["src/both.js"] or 0,
        before_conflict = #diffs["new.js"],
        starts = starts,
        jumps = { ds.jump(starts, 0, 1), ds.jump(starts, 2, 1), ds.jump(starts, 4, 1), ds.jump(starts, 6, 1) or 0, ds.jump(starts, 4, -1), ds.jump(starts, 3, -1), ds.jump(starts, 1, -1) or 0 },
        input = ds.input({ { path = "src/a.js", additions = 1, deletions = 1 } }, diffs, hunks),
        partial = partial,
        progress = progress,
      }))
    `,
  });
  const result = JSON.parse(out);
  assert.deepEqual(result.titles, ["Add the field", "Show it", "Other changes", "Generated files"]);
  assert.deepEqual(result.files, [["src/model.js", "src/model.test.js"], ["src/page.js"], ["README.md"], ["pnpm-lock.yaml"]], "each once, unknown ones dropped, generated last");
  assert.deepEqual(result.place, { "src/model.js": 1, "src/model.test.js": 2, "src/page.js": 3, "README.md": 4, "pnpm-lock.yaml": 5 });
  assert.deepEqual(result.heading, [[["", "Normal"]], [["2/4 Show it", "Title"]], [["  The page shows it, a }", "Comment"]], [["  and all.", "Comment"]]], "its number and title, then its why, a brace in it kept");
  assert.deepEqual(result.narrow.map((l) => l[0][0]), ["1/1 Check the", "    one-worktree", "    cleanup mode", "  The --worktree", "  path must keep", "  the same", "  safety rules", "  everywhere."], "a long title and why wrapped to the list's width, none running off its side");
  assert.ok(result.narrow.every((l) => l[0][0].length <= 16));
  assert.equal(result.bad, true, "no story from an answer that isn't one");
  assert.equal(result.renamed, true, "a renamed file by its new path");
  assert.equal(result.conflicted, 3, "a conflicted file's combined diff by its path");
  assert.equal(result.before_conflict, 2, "and not under the file before it");
  assert.deepEqual(result.starts, [true, true, true, false, false, true], "a heading where a chapter starts in each section, a chapter in both headed in both");
  assert.deepEqual(result.jumps, [1, 3, 6, 0, 2, 2, 0], "]c and [c go between the headings as shown, across sections");
  assert.match(result.input, /^<diff>\n### src\/a\.js \(\+1 -1\)\ndiff --git a\/src\/a\.js b\/src\/a\.js\n\[src\/a\.js#1\] @@ -1 \+1 @@\n-x\n\+y\n<\/diff>$/, "a small file's diff kept whole, its hunk labelled, inside one block");
  assert.deepEqual(result.partial, { titles: ['Add the "pick" column', "Default it on"], started: 3 }, "an answer cut short gives its whole titles, unescaped, and says one more has started");
  assert.deepEqual(result.progress, ["* Telling the change as a story   1:15", "  Read 3 files (+12 −4)", '  ✓ 1. Add the "pick" column', "  ✓ 2. Default it on", "  * 3. …"], "the cover: what is read, each chapter named so far ticked, the one being written spinning");
});

test("Haiku reads every hunk of a long file and the commits' messages, and its answer by hunk splits a file between chapters with a note on each", () => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${nvimDir}`, "-l", "/dev/stdin"], {
    encoding: "utf8",
    input: `
      local ds = require("diffstory")
      -- A 600-line file in 20 hunks, a small one, and an untracked one.
      local long = { "diff --git a/src/big.js b/src/big.js", "--- a/src/big.js", "+++ b/src/big.js" }
      for h = 1, 20 do
        table.insert(long, ("@@ -%d,30 +%d,30 @@ function f%d()"):format(h * 100, h * 100, h))
        for i = 1, 30 do table.insert(long, "+" .. string.rep("x", 200) .. i) end
      end
      local small = "diff --git a/src/small.js b/src/small.js\\n@@ -1 +1,2 @@\\n-a\\n+b\\n+b2\\n"
      local staged = table.concat(long, "\\n") .. "\\n" .. small
      local unstaged = "diff --git a/src/small.js b/src/small.js\\n@@ -5,0 +6,2 @@\\n+c\\n+d\\n"
      local diffs, hunks = ds.split({ staged, unstaged })
      local raw = "aaa\\0Add the field\\0Because the API needs it.\\n\\nCo-Authored-By: X <x@y>\\n\\30\\nbbb\\0Fix it\\0" .. string.rep("y", 2000) .. "\\30\\n"
      local commits = ds.commits(raw)
      ds.TOTAL_CHARS = 20000
      local input = ds.input({ { path = "src/big.js", additions = 600, deletions = 0 }, { path = "src/small.js", additions = 3, deletions = 1 } }, diffs, hunks, commits)
      local answer = vim.json.encode({ chapters = {
        { title = "Add the field", why = "It adds the field. " .. string.rep("Because the API reads it on every call. ", 20), hunks = { { id = "src/big.js#1", note = "Check the default." }, { id = "src/small.js#2", note = "" } } },
        { title = "Show it", why = "The page shows it.", hunks = { { id = "src/small.js#1", note = "Renders it." }, "src/big.js#2" } },
      } })
      local story = ds.parse(answer, { "src/big.js", "src/small.js" }, function() return false end, hunks)
      io.write(vim.json.encode({
        runs = vim.tbl_map(function(h) return { h.id, h.run, h.new_start, h.new_count, h.size } end, hunks["src/small.js"]),
        big = #hunks["src/big.js"],
        input = input,
        commits = commits,
        keys = { ds.key("/r", { "a" }, "diff", commits), ds.key("/r", { "a" }, "diff", ds.commits("aaa\\0Add it\\0\\30")), ds.key("/r", { "a" }, "diff", commits) },
        files = vim.tbl_map(function(c) return c.files end, story.chapters),
        touches = vim.tbl_map(function(c) return c.touches end, story.chapters),
        notes = vim.tbl_map(function(h) return { h.id, h.chapter, h.note } end, story.hunks["src/big.js"]),
        why = story.chapters[1].why,
        short = ds.heading(story, 1, 80, true),
        other = ds.note_lines(story, story.hunks["src/big.js"][2], 1, 80),
        plain = ds.note_lines(story, story.hunks["src/big.js"][3], 1, 80) == nil,
        markdown = ds.markdown(story),
      }))
    `,
  });
  const result = JSON.parse(out);
  assert.deepEqual(result.runs, [["src/small.js#1", 1, 1, 2, 3], ["src/small.js#2", 2, 6, 2, 2]], "a file's hunks numbered across the staged and unstaged runs, each knowing its run");
  assert.equal(result.big, 20);
  assert.ok(result.input.length <= 20000, `the input stays under TOTAL_CHARS (${result.input.length})`);
  for (let h = 1; h <= 20; h += 1) assert.ok(result.input.includes(`[src/big.js#${h}] @@ -${h * 100},30`), `hunk ${h}'s header is kept`);
  assert.match(result.input, /… \d+ more lines in this hunk/, "a long file's hunks cut short, saying so");
  assert.match(result.input, /\[src\/small\.js#1\] @@ -1 \+1,2 @@\n-a\n\+b/, "a small file kept whole");
  assert.ok(result.input.startsWith("<commits>\n- Add the field\n  Because the API needs it.\n- Fix it\n"), "the commits' messages first");
  assert.doesNotMatch(result.commits, /Co-Authored-By/, "trailers left out");
  assert.ok(result.commits.length < 1600 + 60, "a long body cut to COMMIT_CHARS");
  assert.notEqual(result.keys[0], result.keys[1], "a reworded commit is a new story");
  assert.equal(result.keys[0], result.keys[2]);
  assert.deepEqual(result.files, [["src/big.js"], ["src/small.js"]], "each file in the chapter holding most of its changed lines");
  assert.deepEqual(result.touches, [["src/big.js", "src/small.js"], ["src/small.js", "src/big.js"]], "and touching every chapter it has a hunk in");
  assert.deepEqual(result.notes.slice(0, 3), [["src/big.js#1", 1, "Check the default."], ["src/big.js#2", 2, ""], ["src/big.js#3", 1, ""]], "each hunk's chapter and note, one left out in its file's chapter");
  assert.ok(result.why.length <= 400 && result.why.endsWith("call."), "a long why cut at a sentence's end");
  assert.equal(result.short.at(-1)[0][0], "  It adds the field. …", "a narrow list heads a chapter with its first sentence");
  assert.deepEqual(result.other, [[["▎ 2/2 Show it", "Comment"]]], "a hunk of another chapter is named so");
  assert.equal(result.plain, true, "a hunk of the file's chapter with no note has none");
  assert.match(result.markdown, /^### 1\. Add the field\n\nIt adds the field\./);
  assert.match(result.markdown, /- `src\/big\.js`\n  - `src\/big\.js:100` Check the default\./);
  assert.match(result.markdown, /### 2\. Show it\n\nThe page shows it\.\n\n- `src\/small\.js`\n  - `src\/small\.js:1` Renders it\./);
});

test("M-d's cover shows each chapter as Haiku names it, then the diff in the story's order", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const repo = path.join(tmp, "stream-repo");
  const bin = path.join(tmp, "stream-bin");
  const home = path.join(tmp, "stream-home");
  [path.join(repo, "src"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "stream-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  ["a", "b", "c"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "x\n"));
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  ["a", "b", "c"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "y\n"));
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "two"]);
  // A stand-in claude streaming its answer a chapter at a time, a second apart, then the whole of it.
  const pieces = [
    '{"chapters": [{"title": "Change c", "why": "First.", "files": ["src/c.js"]}',
    ', {"title": "Change a", "why": "Second.", "files": ["src/a.js"]}',
    ', {"title": "Change b", "why": "Third.", "files": ["src/b.js"]}]}',
  ];
  const lines = pieces.map((text) => JSON.stringify({ type: "stream_event", event: { type: "content_block_delta", index: 0, delta: { type: "text_delta", text } } }));
  lines.push(JSON.stringify({ type: "result", result: pieces.join("") }));
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\n${lines.map((line) => `sleep 1\nprintf '%s\\n' '${line}'\n`).join("")}`, { mode: 0o755 });
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", `lua
      -- The chapters the cover names, each time they change.
      local seen, last, raw = {}, nil, nil
      vim.wait(8000, function()
        local text
        for _, w in ipairs(vim.api.nvim_list_wins()) do
          local shown = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n")
          if vim.api.nvim_win_get_config(w).relative ~= "" and shown:find("Telling the change", 1, true) then text = shown end
        end
        if not text then return #seen > 0 end
        local named = {}
        for title in text:gmatch("%d%. (Change %a)") do table.insert(named, title) end
        local now = table.concat(named, ",")
        if now ~= last and now ~= "" then table.insert(seen, now) end
        last = now
        if #named == 2 and not raw then
          vim.api.nvim_feedkeys("t", "x", false)
          for _, w in ipairs(vim.api.nvim_list_wins()) do
            local shown = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n")
            if shown:find("answer so far", 1, true) then raw = shown end
          end
          vim.api.nvim_feedkeys("t", "x", false)
        end
        return false
      end, 50)
      local panel = require("diffview.lib").get_current_view().panel
      local covered = false
      for _, w in ipairs(vim.api.nvim_list_wins()) do
        if vim.api.nvim_win_get_config(w).relative ~= "" and table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n"):find("Telling the change", 1, true) then covered = true end
      end
      io.write(vim.json.encode({ seen = seen, raw = raw or "", covered = covered, order = vim.tbl_map(function(e) return e.path end, panel:ordered_file_list()) }))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "stream-cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
  );
  const result = JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.seen, ["Change c", "Change c,Change a", "Change c,Change a,Change b"], "each chapter on the cover as it is named");
  assert.match(result.raw, /"title": "Change a"/, "t shows Haiku's answer itself so far");
  assert.equal(result.covered, false, "the cover goes once the story is told");
  assert.deepEqual(result.order, ["src/c.js", "src/a.js", "src/b.js"], "and the list is in its order");
});

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
  // Its answer as Claude streams one, the whole of it in the final result.
  const streamed = JSON.stringify({ type: "result", result: answer });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > '${given}'\nsleep 2\nprintf '%s\\n' '${streamed}'\n`, { mode: 0o755 });
  // A headless Neovim run with -c writes io.write to stderr, so both are read.
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", "sleep 1000m", "-c", `lua
      local panel = require("diffview.lib").get_current_view().panel
      local function covered()
        for _, w in ipairs(vim.api.nvim_list_wins()) do
          if vim.api.nvim_win_get_config(w).relative ~= "" and table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n"):find("Telling the change", 1, true) then return true end
        end
        return false
      end
      vim.g.covered = covered()
      -- The cover holds still: keys and the wheel leave its cursor at its top, hidden.
      vim.wait(3000, covered, 20)
      vim.api.nvim_feedkeys(vim.keycode("jjjlG<ScrollWheelDown>"), "x", false)
      vim.wait(250)
      vim.g.held = { cursor = vim.api.nvim_win_get_cursor(0), top = vim.fn.line("w0"), guicursor = vim.o.guicursor }
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
      local opened = vim.api.nvim_win_get_width(panel.winid)
      vim.api.nvim_feedkeys("E", "x", false)
      local wide = vim.api.nvim_win_get_width(panel.winid)
      vim.api.nvim_feedkeys("E", "x", false)
      local narrow = vim.api.nvim_win_get_width(panel.winid)
      -- ]p and [p: the story a chapter at a time, each a diffview of its own.
      local function shown()
        local v = require("diffview.lib").get_current_view()
        return { page = v.page and v.page.index or 0, files = vim.tbl_map(function(e) return e.path end, v.panel:ordered_file_list()) }
      end
      local pages = {}
      for _, key in ipairs({ "]p", "]p", "]p", "[p", "[p", "[p" }) do
        vim.api.nvim_feedkeys(vim.keycode(key), "x", false)
        vim.wait(3000, function()
          local v = require("diffview.lib").get_current_view()
          return v and v.panel and #v.panel:ordered_file_list() > 0
        end, 20)
        vim.wait(300)
        table.insert(pages, shown())
      end
      local spinning = 0
      for _, m in ipairs(vim.api.nvim_buf_get_extmarks(panel.bufid, -1, 0, -1, { details = true })) do
        local text = m[4].virt_lines and m[4].virt_lines[1][1] and m[4].virt_lines[1][1][1] or ""
        if text:find("telling the story", 1, true) then spinning = spinning + 1 end
      end
      local uncovered = true
      for _, w in ipairs(vim.api.nvim_list_wins()) do
        if vim.api.nvim_win_get_config(w).relative ~= "" and table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n"):find("Telling the change", 1, true) then uncovered = false end
      end
      io.write(vim.json.encode({ covered = vim.g.covered, held = vim.g.held, shown = vim.o.guicursor ~= "a:SwitchboardHiddenCursor", uncovered = uncovered, pages = pages, telling = vim.g.telling, spinning = spinning, order = vim.tbl_map(function(e) return e.path end, panel:ordered_file_list()), marks = marks, bars = bars, jumped = jumped, opened = opened, wide = wide, narrow = narrow }))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
  );
  const result = JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.order, ["src/model.js", "test/model.test.js", "src/ui.js"], "the test beside its code, in the story's order");
  assert.deepEqual(result.marks.map((m) => [m.line.split(" ")[1], m.title]), [["model.js", "1/2 Change the model"], ["ui.js", "2/2 Show it"]], "each chapter headed over its first file");
  assert.ok(result.bars.some((bar) => bar.includes("1/2 Change the model · ")), "the chapter in the bar");
  assert.match(result.jumped, /ui\.js/, "]c goes to the next chapter");
  assert.deepEqual(result.pages, [
    { page: 1, files: ["src/model.js", "test/model.test.js"] },
    { page: 2, files: ["src/ui.js"] },
    { page: 2, files: ["src/ui.js"] },
    { page: 1, files: ["src/model.js", "test/model.test.js"] },
    { page: 0, files: ["src/model.js", "test/model.test.js", "src/ui.js"] },
    { page: 0, files: ["src/model.js", "test/model.test.js", "src/ui.js"] },
  ], "]p turns to each chapter's own page and stops at the last; [p goes back, and from the first to the whole diff");
  assert.equal(result.opened, 32, "the list opens wide enough for its longest path");
  assert.deepEqual([result.wide, result.narrow], [30, 32], "E narrows it, and again widens it");
  assert.equal(result.covered, true, "the diff waits under a cover while its story is told");
  assert.deepEqual(result.held, { cursor: [1, 0], top: 1, guicursor: "a:SwitchboardHiddenCursor" }, "the cover's cursor hidden and kept at its top whatever is pressed");
  assert.equal(result.shown, true, "the cursor shown again once the cover goes");
  assert.equal(result.uncovered, true, "and is shown once it is");
  assert.equal(result.telling.length, 1, "while Haiku reads the change, the list says so");
  assert.match(result.telling[0], /telling the story/);
  assert.equal(result.spinning, 0, "and stops saying so once the story is known");
  assert.match(fs.readFileSync(given, "utf8"), /<diff>[\s\S]*### src\/model\.js \(\+1 -1\)[\s\S]*\+A/, "Haiku read the diff itself");
});

test("M-d draws each hunk's note over it in the diff, naming the chapter of a hunk that belongs to another, and Haiku reads the commits", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const repo = path.join(tmp, "notes-repo");
  const bin = path.join(tmp, "notes-bin");
  const home = path.join(tmp, "notes-home");
  [path.join(repo, "src"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "notes-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  const lines = Array.from({ length: 20 }, (_, i) => `line ${i + 1}`);
  execFileSync("git", ["-C", repo, "init", "-q"]);
  fs.writeFileSync(path.join(repo, "src/a.js"), `${lines.join("\n")}\n`);
  fs.writeFileSync(path.join(repo, "src/b.js"), "b\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  // Two hunks in a.js, at lines 3 and 17, which with two lines of context start at 1 and 15.
  fs.writeFileSync(path.join(repo, "src/a.js"), `${lines.map((line, i) => (i === 2 || i === 16 ? `${line} changed` : line)).join("\n")}\n`);
  fs.writeFileSync(path.join(repo, "src/b.js"), "B\n");
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "Change a and b", "-m", "So the page can show them."]);
  const given = path.join(tmp, "notes-given.txt");
  const answer = JSON.stringify({ chapters: [
    { title: "Change a", why: "It changes a.", hunks: [{ id: "src/a.js#1", note: "First change." }] },
    { title: "Show it", why: "The page shows it.", hunks: [{ id: "src/a.js#2", note: "Second." }, { id: "src/b.js#1", note: "" }] },
  ] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > '${given}'\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", `lua
      vim.wait(8000, function() local v = require("diffview.lib").get_current_view() return v.story ~= nil end, 50)
      local view = require("diffview.lib").get_current_view()
      local a = vim.tbl_filter(function(e) return e.path == "src/a.js" end, view.panel:ordered_file_list())[1]
      view:set_file(a, false)
      vim.wait(2000, function() return view.cur_entry == a end, 20)
      vim.wait(300)
      local ns = vim.api.nvim_create_namespace("diff_story_notes")
      local sides = {}
      for _, w in ipairs(view.cur_layout.windows) do
        local marks = {}
        for _, m in ipairs(vim.api.nvim_buf_get_extmarks(vim.api.nvim_win_get_buf(w.id), ns, 0, -1, { details = true })) do
          table.insert(marks, { m[2], m[4].virt_lines and m[4].virt_lines[1][1][1] or "" })
        end
        sides[w.file.symbol] = marks
      end
      io.write(vim.json.encode(sides))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "notes-cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
  );
  const result = JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.b, [[0, "▎ First change."], [14, "▎ 2/2 Show it · Second."]], "each note over its hunk's first line on the right, a hunk of another chapter named so");
  assert.deepEqual(result.a, [[0, ""], [14, ""]], "with as many blank lines on the left, so both stay level");
  const read = fs.readFileSync(given, "utf8");
  assert.match(read, /^<commits>\n- Change a and b\n {2}So the page can show them\.\n<\/commits>/, "Haiku read the commit's message");
  assert.match(read, /\[src\/a\.js#2\] @@ -15,5 \+15,5 @@/, "and each hunk by its id");
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
  // Untracked, and in the second chapter: not on the first's page.
  fs.writeFileSync(path.join(repo, "src/extra.js"), "E\n");
  const answer = JSON.stringify({ chapters: [{ title: "Change the model", why: "The data changes shape.", files: ["src/model.js", "test/model.test.js"] }, { title: "Show it", why: "The page shows it.", files: ["src/ui.js", "src/extra.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "--cmd", "let g:diff_list_wide = v:false", "-c", "DiffviewOpen", "-c", "sleep 4000m", "-c", `lua
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
      vim.api.nvim_feedkeys(vim.keycode("]p"), "x", false)
      vim.wait(3000, function()
        local v = require("diffview.lib").get_current_view()
        return v.page ~= nil and #v.panel:ordered_file_list() > 0
      end, 20)
      vim.wait(500)
      local shown = require("diffview.lib").get_current_view()
      local page = vim.tbl_map(function(e) return e.path end, shown.panel:ordered_file_list())
      io.write(vim.json.encode({ headed = headed, next = next, back = back, page = page, opened = shown.cur_entry and shown.cur_entry.path, width = vim.api.nvim_win_get_width(panel.winid) }))`, "-c", "qa"],
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
  assert.notEqual(result.opened, "src/extra.js", "nor does it open on one");
  assert.equal(result.width, 30, "with the diff_list_wide setting off, the list opens narrow");
  assert.deepEqual(result.page.sort(), ["src/model.js", "src/model.js", "test/model.test.js"], "the first page holds its chapter's files in both sections, no other chapter's untracked one");
});

test("a branch's story is told ahead of time by a hidden Neovim, kept where M-d finds it, once per commit", { skip: !ready && "needs lazy.nvim's diffview", timeout: 120_000 }, () => {
  const root = path.join(tmp, "ahead");
  const home = path.join(root, "home");
  const bin = path.join(root, "bin");
  const origin = path.join(root, "origin.git");
  const repo = path.join(root, "repo");
  [path.join(home, ".config"), bin].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  fs.symlinkSync(nvimDir, path.join(home, ".config/nvim"));
  const asked = path.join(root, "asked");
  const answer = JSON.stringify({ chapters: [{ title: "Change the model", why: "The data changes shape.", files: ["src/model.js"] }, { title: "Show it", why: "The page shows it.", files: ["src/ui.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\necho "x $$ $PPID $(echo "$@" | cut -c1-80)" >> '${asked}'\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, "gh"), "#!/bin/sh\necho main\n", { mode: 0o755 });
  const env = { ...process.env, HOME: home, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(root, "cache"), XDG_STATE_HOME: path.join(root, "state"), XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "", GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], { env, stdio: "ignore" });
  execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
  execFileSync("git", ["clone", "-q", origin, repo], { env, stdio: "ignore" });
  fs.mkdirSync(path.join(repo, "src"));
  fs.writeFileSync(path.join(repo, "src/model.js"), "a\n");
  fs.writeFileSync(path.join(repo, "src/ui.js"), "b\n");
  git("add", ".");
  git("commit", "-qm", "one");
  git("push", "-q", "origin", "HEAD:main");
  git("checkout", "-q", "-b", "feature");
  fs.writeFileSync(path.join(repo, "src/model.js"), "A\n");
  fs.writeFileSync(path.join(repo, "src/ui.js"), "B\n");
  git("commit", "-qam", "two");
  const ahead = () => spawnSync("bash", [path.join(__dirname, "../tmux/diff-story-ahead.sh"), repo], { env, encoding: "utf8", timeout: 100_000 });
  // Claude down first: nothing kept, and the commit left to tell again.
  const working = fs.readFileSync(path.join(bin, "claude"), "utf8");
  fs.writeFileSync(path.join(bin, "claude"), "#!/bin/sh\ncat >/dev/null\nexit 1\n", { mode: 0o755 });
  assert.notEqual(ahead().status, 0, "a story that couldn't be told is said so");
  fs.writeFileSync(path.join(bin, "claude"), working, { mode: 0o755 });
  assert.equal(ahead().status, 0, "told once Claude answers");
  const kept = fs.readdirSync(path.join(root, "cache/nvim/diff-story"));
  assert.equal(kept.length, 1, "the story kept in Neovim's cache, where M-d looks");
  assert.match(fs.readFileSync(path.join(root, "cache/nvim/diff-story", kept[0]), "utf8"), /Change the model/);
  ahead();
  assert.equal(fs.readFileSync(asked, "utf8").trim().split("\n").length, 1, "told once for the commit");
});

test("M-d opened while another Neovim tells the same story follows that ask rather than asking again", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, async () => {
  const repo = path.join(tmp, "shared-repo");
  const bin = path.join(tmp, "shared-bin");
  const home = path.join(tmp, "shared-home");
  [path.join(repo, "src"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "shared-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  ["a", "b"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "x\n"));
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  ["a", "b"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "y\n"));
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "two"]);
  // A slow stand-in claude that notes each time it is asked.
  const asked = path.join(tmp, "shared-asked");
  const answer = JSON.stringify({ chapters: [{ title: "Change b", why: "First.", files: ["src/b.js"] }, { title: "Change a", why: "Then.", files: ["src/a.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\necho asked >> '${asked}'\nsleep 3\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "shared-cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" };
  // The first Neovim asks, as the story told ahead of time would.
  const first = spawn("nvim", ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", "sleep 6000m", "-c", "qa"], { cwd: repo, env, stdio: "ignore" });
  const firstDone = new Promise((resolve) => first.on("close", resolve));
  await new Promise((resolve) => setTimeout(resolve, 1500));
  const second = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", `lua
      local view = require("diffview.lib").get_current_view()
      vim.wait(8000, function() return view.story ~= nil end, 50)
      io.write(vim.json.encode({ order = vim.tbl_map(function(e) return e.path end, view.panel:ordered_file_list()) }))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env },
  );
  await firstDone;
  const result = JSON.parse(`${second.stdout}${second.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.order, ["src/b.js", "src/a.js"], "the second shows the story the first was told");
  assert.equal(fs.readFileSync(asked, "utf8").trim().split("\n").length, 1, "and Haiku was asked once");
  assert.deepEqual(fs.readdirSync(path.join(tmp, "shared-cache/nvim/diff-story")).filter((name) => name.endsWith(".partial")), [], "no progress left behind");
});

test("an ask whose Neovim is gone, its progress not updated for 30 seconds, is asked again", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, async () => {
  const repo = path.join(tmp, "stale-repo");
  const bin = path.join(tmp, "stale-bin");
  const home = path.join(tmp, "stale-home");
  [path.join(repo, "src"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "stale-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  ["a", "b"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "x\n"));
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  ["a", "b"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "y\n"));
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "two"]);
  const asked = path.join(tmp, "stale-asked");
  const answer = JSON.stringify({ chapters: [{ title: "Change b", why: "First.", files: ["src/b.js"] }, { title: "Change a", why: "Then.", files: ["src/a.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\necho asked >> '${asked}'\nsleep 2\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "stale-cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" };
  // A Neovim killed mid-ask leaves its progress behind, which then ages.
  const stories = path.join(tmp, "stale-cache/nvim/diff-story");
  const first = spawn("nvim", ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", "sleep 6000m", "-c", "qa"], { cwd: repo, env, stdio: "ignore" });
  for (let i = 0; i < 50 && !(fs.existsSync(stories) && fs.readdirSync(stories).some((name) => name.endsWith(".partial"))); i += 1) await new Promise((resolve) => setTimeout(resolve, 100));
  first.kill("SIGKILL");
  const left = fs.readdirSync(stories).filter((name) => name.endsWith(".partial"));
  assert.equal(left.length, 1, "the killed Neovim's progress is left");
  const old = new Date(Date.now() - 40_000);
  fs.utimesSync(path.join(stories, left[0]), old, old);
  const second = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", `lua
      local view = require("diffview.lib").get_current_view()
      vim.wait(8000, function() return view.story ~= nil end, 50)
      io.write(vim.json.encode({ order = vim.tbl_map(function(e) return e.path end, view.panel:ordered_file_list()) }))`, "-c", "qa"],
    { cwd: repo, encoding: "utf8", env },
  );
  const result = JSON.parse(`${second.stdout}${second.stderr}`.trim().split("\n").at(-1));
  assert.deepEqual(result.order, ["src/b.js", "src/a.js"], "the story is told");
  assert.equal(fs.readFileSync(asked, "utf8").trim().split("\n").length, 2, "by asking Haiku again");
});

test("easy mode puts a button for each of the list's keys at the bottom of M-d's diff, with the keys named after them unless the help line is off", { skip: !ready && "diffview isn't installed" }, () => {
  const home = path.join(tmp, "easy-home");
  const config = path.join(tmp, "easy-config");
  [home, config].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  // Each case starts M-d's Neovim with the settings given as --cmd, and reads
  // its statusline as it starts.
  [
    { cmds: ["--cmd", "let g:diff_easy = v:true"], buttons: 7, help: true, laststatus: "3" },
    { cmds: ["--cmd", "let g:diff_easy = v:true", "--cmd", "let g:diff_key_help = v:false"], buttons: 7, help: false, laststatus: "3" },
    { cmds: [], buttons: 0, help: false, laststatus: "0" },
  ].forEach(({ cmds, buttons, help, laststatus }) => {
    const run = spawnSync("nvim", ["--headless", "-u", path.join(nvimDir, "diff.lua"), ...cmds, "-c", "lua io.write(vim.o.laststatus .. '\\n' .. vim.o.statusline .. '\\n')", "-c", "qa!"], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, XDG_CONFIG_HOME: config, XDG_CACHE_HOME: path.join(tmp, "easy-cache"), XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" },
    });
    const [last, line] = (run.stdout + run.stderr).trim().split("\n").slice(-2);
    assert.equal(last, laststatus, `${cmds.join(" ")}: the bar is shown only in easy mode`);
    assert.equal(line.match(/@v:lua\.diff_button_click@/g)?.length ?? 0, buttons);
    if (buttons) ["Chapter ▶", "Page ▶", "Widen", "Copy link", "Close"].forEach((label) => assert.ok(line.includes(` ${label} `), label));
    assert.equal(/\]c \[c chapters/.test(line), help, "the keys are named unless the help line is off");
  });
});
