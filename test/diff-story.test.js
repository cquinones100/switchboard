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

test("the cover says about how long the story should take, from the diff's hunks", () => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${nvimDir}`, "-l", "/dev/stdin"], {
    encoding: "utf8",
    input: `
      local ds = require("diffstory")
      local lines = {}
      for h = 1, 3 do vim.list_extend(lines, { "diff --git a/f" .. h .. ".js b/f" .. h .. ".js", "@@ -1 +1 @@", "-a", "+b", "@@ -9 +9 @@", "-c", "+d" }) end
      local _, hunks = ds.split(table.concat(lines, "\\n") .. "\\n")
      io.write(vim.json.encode({
        count = ds.count(hunks),
        estimates = { ds.estimate(0), ds.estimate(100), ds.estimate(474) },
        cover = ds.progress({ text = "", started = 100 }, { files = 3, additions = 6, deletions = 6, estimate = 153 }, "*", 142)[1],
        before = ds.progress(nil, { files = 3, additions = 6, deletions = 6, estimate = 153 }, "*", 142)[1],
      }))
    `,
  });
  const result = JSON.parse(out);
  assert.equal(result.count, 6, "every hunk of every file");
  assert.deepEqual(result.estimates, [25, 52, 153], "a start and a share per hunk");
  assert.equal(result.cover, "* Telling the change as a story   0:42 of about 2:33", "the time taken and the time it should take");
  assert.equal(result.before, "* Telling the change as a story", "nothing timed before the ask starts");
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

test("An id written without the folders every path shares is the one path ending in it; one two paths end in is dropped; an answer mostly missing says so", () => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${nvimDir}`, "-l", "/dev/stdin"], {
    encoding: "utf8",
    input: `
      local ds = require("diffstory")
      local function file(p, n)
        local lines = { "diff --git a/" .. p .. " b/" .. p }
        for h = 1, n do table.insert(lines, ("@@ -%d +%d @@\\n-a\\n+b"):format(h * 10, h * 10)) end
        return table.concat(lines, "\\n") .. "\\n"
      end
      local paths = { "pkgs/web-admin/lib/format.ts", "pkgs/web-admin/app/page.tsx", "pkgs/web-admin/lib/util.ts", "pkgs/api/lib/util.ts", "lib/page.tsx", "pkgs/web-admin/lib/page.tsx" }
      local _, hunks = ds.split(table.concat(vim.tbl_map(function(p) return file(p, 2) end, paths)))
      local none = function() return false end
      local short = ds.parse(vim.json.encode({ chapters = {
        { title = "Format the dates", why = "", hunks = { { id = "lib/format.ts#1", note = "Check the zone." }, "lib/format.ts#2", "lib/util.ts#1" } },
        { title = "Show them", why = "", hunks = { { id = "app/page.tsx#2", note = "Renders it." }, "web-admin/app/page.tsx#1", "lib/page.tsx#1", "lib/page.tsx#9" } },
      } }), vim.list_extend({ "pkgs/web-admin/lib/format.ts" }, paths), none, hunks)
      local missing = ds.parse(vim.json.encode({ chapters = {
        { title = "One", why = "", hunks = { "pkgs/web-admin/lib/format.ts#1", "nope.ts#1", "other/thing.ts#1", "x.ts#2" } },
      } }), paths, none, hunks)
      local nothing, why = ds.parse(vim.json.encode({ chapters = { { title = "Util", why = "", hunks = { "lib/util.ts#1" } } } }), paths, none, hunks)
      local _, made_why = ds.parse(vim.json.encode({ chapters = { { title = "Lock", why = "", hunks = { "pkgs/web-admin/lib/util.ts#1" } } } }), paths, function(p) return p:match("util") ~= nil end, hunks)
      local function where(story, p) return vim.tbl_map(function(h) return { h.chapter, h.note } end, story.hunks[p]) end
      io.write(vim.json.encode({
        files = vim.tbl_map(function(c) return { c.title, c.files } end, short.chapters),
        format = where(short, "pkgs/web-admin/lib/format.ts"),
        page = where(short, "pkgs/web-admin/app/page.tsx"),
        exact = where(short, "lib/page.tsx"),
        nested = where(short, "pkgs/web-admin/lib/page.tsx"),
        short_missed = short.missed or vim.NIL,
        missed = missing.missed,
        nothing = nothing == nil,
        why = why,
        made_why = made_why or vim.NIL,
      }))
    `,
  });
  const result = JSON.parse(out);
  assert.deepEqual(result.format, [[1, "Check the zone."], [1, ""]], "ids without the shared folder land in their chapter, notes included, a path listed twice (staged and unstaged) still one path");
  assert.deepEqual(result.page, [[2, ""], [2, "Renders it."]], "a path missing any of its leading folders is found");
  assert.deepEqual(result.exact, [[2, ""], [2, ""]], "an exact id wins over a path that ends in it");
  assert.deepEqual(result.nested[0], [3, ""], "a file named only by a path another file has exactly isn't taken");
  assert.deepEqual(result.files, [
    ["Format the dates", ["pkgs/web-admin/lib/format.ts"]],
    ["Show them", ["pkgs/web-admin/app/page.tsx", "lib/page.tsx"]],
    ["Other changes", ["pkgs/api/lib/util.ts", "pkgs/web-admin/lib/page.tsx", "pkgs/web-admin/lib/util.ts"]],
  ], "a shortened path two files end in is dropped, not guessed");
  assert.equal(result.short_missed, null, "an answer whose ids are found says nothing");
  assert.deepEqual(result.missed, { given: 4, matched: 1 }, "an answer whose ids mostly miss says so");
  assert.equal(result.nothing, true, "an answer none of whose ids are found is no story");
  assert.equal(result.why, "Haiku's answer named hunks this diff doesn't have (0 of 1 ids matched)", "and says why");
  assert.equal(result.made_why, null, "an answer naming only generated files' hunks isn't said to name hunks the diff doesn't have");
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

test("M-d covers the diff as it opens, blank at first, and opens a story it has told before with no cover", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const repo = path.join(tmp, "blank-repo");
  const bin = path.join(tmp, "blank-bin");
  const home = path.join(tmp, "blank-home");
  const cache = path.join(tmp, "blank-cache");
  [path.join(repo, "src"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "blank-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  ["a", "b", "c"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "x\n"));
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  ["a", "b", "c"].forEach((name) => fs.writeFileSync(path.join(repo, `src/${name}.js`), "y\n"));
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "two"]);
  // A stand-in claude that answers after a second, noting each time it is asked.
  const asked = path.join(tmp, "blank-asked");
  const answer = JSON.stringify({ chapters: [{ title: "Change c", why: "First.", files: ["src/c.js"] }, { title: "Change a", why: "Second.", files: ["src/a.js"] }, { title: "Change b", why: "Third.", files: ["src/b.js"] }] });
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\ncat > /dev/null\necho asked >> '${asked}'\nsleep 1\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  // Watches M-d from the moment it opens: whether the cover is in front, its
  // words, each order the list is seen in, until the story settles.
  const open = () => {
    const run = spawnSync(
      "nvim",
      ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1...HEAD", "-c", `lua
        local function cover_text()
          for _, w in ipairs(vim.api.nvim_list_wins()) do
            if vim.api.nvim_win_get_config(w).relative ~= "" then return table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n") end
          end
        end
        local first = { front = vim.api.nvim_win_get_config(0).relative ~= "", text = cover_text() }
        local view = require("diffview.lib").get_current_view()
        local orders, last, covered, worded, behind = {}, nil, first.text ~= nil, false, false
        local function look()
          local text = cover_text()
          if text then covered = true end
          if text and text:find("Telling the change", 1, true) then worded = true end
          -- No diff window in front while the cover is up.
          if text and vim.api.nvim_win_get_config(0).relative == "" then behind = true end
          local order = table.concat(vim.tbl_map(function(e) return e.path end, view.panel:ordered_file_list()), ",")
          if order ~= "" and order ~= last then table.insert(orders, order) end
          last = order
        end
        vim.wait(150, function() look() return false end, 10)
        local blank = cover_text()
        vim.wait(8000, function() look() return view.settled and not cover_text() end, 10)
        local index = io.open(vim.fn.stdpath("cache") .. "/diff-story/index.json")
        io.write(vim.json.encode({ first = first, blank = blank or vim.NIL, covered = covered, worded = worded, behind = behind, orders = orders, index = index and index:read("*a") or "" }))`, "-c", "qa"],
      { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: cache, HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
    );
    return JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  };
  const told = open();
  assert.deepEqual(told.first, { front: true, text: "" }, "the cover is in front, blank, as the view opens");
  assert.equal(told.blank, "", "and still blank 150ms on");
  assert.equal(told.behind, false, "no diff window comes in front of it");
  assert.equal(told.worded, true, "it says so once the story takes a moment");
  assert.equal(told.orders.at(-1), "src/c.js,src/a.js,src/b.js", "then the list is in the story's order");
  const index = JSON.parse(told.index);
  const [at, key] = Object.entries(index)[0];
  assert.match(at, /\u0000[0-9a-f]{40}\u0000[0-9a-f]{40}$/, "the index knows the diff by its two commits");
  assert.ok(fs.existsSync(path.join(cache, "nvim/diff-story", `${key}.json`)), "and keeps the story beside it");

  const again = open();
  assert.equal(again.covered, false, "a story told before opens with no cover");
  assert.deepEqual(again.orders, ["src/c.js,src/a.js,src/b.js"], "its list in the story's order from the start");
  assert.equal(fs.readFileSync(asked, "utf8").trim().split("\n").length, 1, "and Haiku isn't asked again");

  // The index pointing at another story: that one shown first, then the cover while the right one is told.
  const dir = path.join(cache, "nvim/diff-story");
  fs.writeFileSync(path.join(dir, "stale.json"), JSON.stringify({ chapters: [{ title: "Old", why: "", files: ["src/b.js", "src/a.js", "src/c.js"], touches: ["src/b.js", "src/a.js", "src/c.js"] }], place: { "src/b.js": 1, "src/a.js": 2, "src/c.js": 3 }, chapter: { "src/b.js": 1, "src/a.js": 1, "src/c.js": 1 }, hunks: {} }));
  fs.writeFileSync(path.join(dir, "index.json"), JSON.stringify({ [at]: "stale" }));
  fs.rmSync(path.join(dir, `${key}.txt`));
  fs.rmSync(path.join(dir, `${key}.json`));
  const stale = open();
  assert.equal(stale.covered, true, "a stale entry brings the cover up after all");
  assert.equal(stale.orders.at(-1), "src/c.js,src/a.js,src/b.js", "and the right story is told");
  assert.deepEqual(JSON.parse(stale.index), { [at]: key }, "the entry is put right");
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

test("a branch's story is told ahead of time by a hidden Neovim, kept where M-d finds it, once per commit, and cleared from M-p", { skip: !ready && "needs lazy.nvim's diffview", timeout: 120_000 }, () => {
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
  const kept = fs.readdirSync(path.join(root, "cache/nvim/diff-story")).filter((name) => name.endsWith(".txt"));
  assert.equal(kept.length, 1, "the story kept in Neovim's cache, where M-d looks");
  assert.equal(Object.keys(JSON.parse(fs.readFileSync(path.join(root, "cache/nvim/diff-story/index.json"), "utf8"))).length, 1, "and in its index, so M-d opens it with no cover");
  assert.match(fs.readFileSync(path.join(root, "cache/nvim/diff-story", kept[0]), "utf8"), /Change the model/);
  ahead();
  assert.equal(fs.readFileSync(asked, "utf8").trim().split("\n").length, 1, "told once for the commit");
  // M-p's "Clear this tab's diff cache", from a copy whose say.sh keeps what
  // it says, with a tmux that answers nothing, so neither reaches a tmux server.
  const tools = path.join(root, "tools");
  fs.mkdirSync(tools);
  fs.copyFileSync(path.join(__dirname, "../tmux/diff-cache-clear.sh"), path.join(tools, "diff-cache-clear.sh"));
  fs.symlinkSync(path.join(__dirname, "../tmux/diff-base.sh"), path.join(tools, "diff-base.sh"));
  fs.writeFileSync(path.join(bin, "tmux"), "#!/bin/sh\nexit 1\n", { mode: 0o755 });
  const said = path.join(root, "said");
  fs.writeFileSync(path.join(tools, "say.sh"), `#!/bin/sh\nprintf '%s\\n' "$2" >> '${said}'\n`, { mode: 0o755 });
  const story = path.join(root, "cache/nvim/diff-story");
  fs.writeFileSync(path.join(story, "unrelated.txt"), "another diff's story\n");
  const clear = () => {
    fs.rmSync(said, { force: true });
    assert.equal(spawnSync("bash", [path.join(tools, "diff-cache-clear.sh"), path.join(repo, "src")], { env, encoding: "utf8", timeout: 60_000 }).status, 0);
    return fs.readFileSync(said, "utf8").trim();
  };
  // While another Neovim tells it, the story is kept, since that telling would keep it again.
  const partial = path.join(story, kept[0].replace(/\.txt$/, ".partial"));
  fs.writeFileSync(partial, "{}");
  assert.equal(fs.readdirSync(path.join(root, "cache/switchboard/diff-base")).length, 1, "the PR base kept for the worktree");
  assert.match(clear(), /^This tab's diff cache is cleared: 1 file\. Its story is being told right now, so it was kept/);
  assert.ok(fs.existsSync(path.join(story, kept[0])), "the story being told kept");
  assert.equal(fs.readdirSync(path.join(root, "cache/switchboard/diff-base")).length, 0, "the PR base gone");
  fs.rmSync(partial);
  assert.equal(clear(), "This tab's diff cache is cleared: 3 files.", "the base asked for again, then cleared with the story");
  assert.deepEqual(fs.readdirSync(story).sort(), ["index.json", "unrelated.txt"], "the diff's story gone, another's kept");
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(story, "index.json"), "utf8")), {}, "and its index entry");
  assert.equal(fs.readdirSync(path.join(root, "cache/switchboard/diff-base")).length, 0, "and the PR base");
  assert.equal(fs.readFileSync(asked, "utf8").trim().split("\n").length, 1, "nothing told again");
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
    { cmds: ["--cmd", "let g:diff_easy = v:true"], buttons: 9, help: true, laststatus: "3" },
    { cmds: ["--cmd", "let g:diff_easy = v:true", "--cmd", "let g:diff_key_help = v:false"], buttons: 9, help: false, laststatus: "3" },
    { cmds: [], buttons: 0, help: false, laststatus: "0" },
  ].forEach(({ cmds, buttons, help, laststatus }) => {
    const run = spawnSync("nvim", ["--headless", "-u", path.join(nvimDir, "diff.lua"), ...cmds, "-c", "lua io.write(vim.o.laststatus .. '\\n' .. vim.o.statusline .. '\\n')", "-c", "qa!"], {
      encoding: "utf8",
      env: { ...process.env, HOME: home, XDG_CONFIG_HOME: config, XDG_CACHE_HOME: path.join(tmp, "easy-cache"), XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" },
    });
    const [last, line] = (run.stdout + run.stderr).trim().split("\n").slice(-2);
    assert.equal(last, laststatus, `${cmds.join(" ")}: the bar is shown only in easy mode`);
    assert.equal(line.match(/@v:lua\.diff_button_click@/g)?.length ?? 0, buttons);
    if (buttons) ["Chapter ▶", "Page ▶", "Widen", "Copy link", "Refresh", "Ask", "Close"].forEach((label) => assert.ok(line.includes(` ${label} `), label));
    assert.equal(/\]c \[c chapters/.test(line), help, "the keys are named unless the help line is off");
  });
});

test("a question about the diff gives Haiku the story, the diff and where the cursor is before the question, and reads its stream", () => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${nvimDir}`, "-l", "/dev/stdin"], {
    encoding: "utf8",
    input: `
      local da = require("diffask")
      local hunks = { ["src/a.js"] = { { id = "src/a.js#1", new_start = 3, new_count = 4 }, { id = "src/a.js#2", new_start = 20, new_count = 0 } } }
      local story = { chapters = { { title = "Change a", why = "It changes a.", files = { "src/a.js" }, touches = { "src/a.js" } } }, hunks = {} }
      local at = { path = "src/a.js", line = 5, side = "b", hunk = da.hunk_at(hunks, "src/a.js", 5).id }
      local texts, tools, result = {}, {}, nil
      local read = da.reader(function(t) table.insert(texts, t) end, function(name, input) table.insert(tools, da.tool_line(name, input, "/repo")) end, function(t) result = t end)
      read('{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"It "}}}')
      read('{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/repo/lib/b.js"}}]}}')
      read('{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"works."}}}')
      read('not json')
      read('{"type":"result","result":"It works."}')
      io.write(vim.json.encode({
        input = da.input(story, "<diff>\\nD\\n</diff>", at, "  Why this?  "),
        none = da.input(nil, nil, { path = "src/a.js" }, "Q"),
        old = da.where({ path = "src/a.js", line = 2, side = "a" }),
        outside = da.hunk_at(hunks, "src/a.js", 8) == nil,
        removed = da.hunk_at(hunks, "src/a.js", 20).id,
        old_side = da.hunk_at({ ["o.js"] = { { id = "o.js#1", old_start = 10, old_count = 2, new_start = 3, new_count = 1 } } }, "o.js", 11, nil, "a").id,
        unstaged = da.hunk_at({ ["s.js"] = { { id = "s.js#1", run = 1, new_start = 1, new_count = 3 }, { id = "s.js#2", run = 2, new_start = 2, new_count = 1 } } }, "s.js", 2, 2).id,
        texts = texts, tools = tools, result = result,
        cited = da.cited("See src/a.js:12 and lib/b.js:3.", 22),
        cited_none = da.cited("See src/a.js:12.", 0) == nil,
        entries = da.entries(require("diffstory").split("diff --git a/b.js b/b.js\\n--- a/b.js\\n+++ b/b.js\\n@@ -1,2 +1,2 @@\\n-x\\n+y\\n+z\\ndiff --git a/a.js b/a.js\\n@@ -1 +0,0 @@\\n-q\\ndiff --git a/b.js b/b.js\\n--- a/b.js\\n+++ b/b.js\\n@@ -5 +5 @@\\n-m\\n+n\\n")),
        cited_svelte = da.cited("At src/routes/+page.svelte:12 and app/@modal/page.tsx:4", 5),
        cited_slot = da.cited("At src/routes/+page.svelte:12 and app/@modal/page.tsx:4", 40),
        cited_route = da.cited("(see src/app/[slug]/(auth)/page.tsx:7)", 10),
      }))
    `,
  });
  const r = JSON.parse(out);
  assert.equal(r.input, "<story>\n### 1. Change a\n\nIt changes a.\n\n- `src/a.js`\n</story>\n\n<diff>\nD\n</diff>\n\n<cursor>\nsrc/a.js:5 in hunk [src/a.js#1]\n</cursor>\n\n<question>\nWhy this?\n</question>", "the story, the diff, the cursor and its hunk, then the question");
  assert.match(r.none, /^<story>\nNo story told for this diff\.\n<\/story>\n\n<diff>\n<\/diff>\n\n<cursor>\nsrc\/a\.js\n<\/cursor>/, "with no story or diff, says so");
  assert.equal(r.old, "src/a.js:2 (the old side, before the change)");
  assert.equal(r.outside, true, "a line past a hunk is in none");
  assert.equal(r.removed, "src/a.js#2", "a hunk that only removes lines holds the line it sits at");
  assert.equal(r.old_side, "o.js#1", "a line of the old side by the hunk's old lines");
  assert.equal(r.unstaged, "s.js#2", "the unstaged change at a line, not the staged one there too");
  assert.deepEqual(r.texts, ["It ", "works."]);
  assert.deepEqual(r.tools, ["_Read lib/b.js_"], "each file it opens, relative to the repository");
  assert.equal(r.result, "It works.");
  assert.deepEqual(r.cited, { path: "lib/b.js", line: 3 }, "the path:line under the cursor");
  assert.equal(r.cited_none, true);
  assert.deepEqual([r.cited_svelte, r.cited_slot], [{ path: "src/routes/+page.svelte", line: 12 }, { path: "app/@modal/page.tsx", line: 4 }], "+ and @ in a path kept");
  assert.deepEqual(r.entries, [{ path: "a.js", additions: 0, deletions: 1 }, { path: "b.js", additions: 3, deletions: 2 }], "each file and its counts from the diff itself, its staged and unstaged diffs both, their headers left out");
  assert.deepEqual(r.cited_route, { path: "src/app/[slug]/(auth)/page.tsx", line: 7 }, "brackets and parentheses in a path kept, an opening one around it left out");
});

test("ga in M-d's diff asks Haiku in the repository with its read-only tools, and streams the answer into a window beside the diff", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const repo = path.join(tmp, "ask-repo");
  const bin = path.join(tmp, "ask-bin");
  const home = path.join(tmp, "ask-home");
  [path.join(repo, "src"), bin, home].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  const config = path.join(tmp, "ask-config");
  fs.mkdirSync(config);
  fs.symlinkSync(nvimDir, path.join(config, "nvim"));
  execFileSync("git", ["-C", repo, "init", "-q"]);
  fs.writeFileSync(path.join(repo, "src/a.js"), "x\n");
  execFileSync("git", ["-C", repo, "add", "."]);
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qm", "one"]);
  fs.writeFileSync(path.join(repo, "src/a.js"), "y\n");
  execFileSync("git", ["-C", repo, "-c", "user.email=t@e", "-c", "user.name=t", "commit", "-qam", "two"]);
  // A stand-in claude: keeps its arguments, directory and input, then streams an answer in two pieces.
  const kept = path.join(tmp, "ask-kept");
  const lines = ["It ", "changes x to y."].map((text) => JSON.stringify({ type: "stream_event", event: { type: "content_block_delta", delta: { type: "text_delta", text } } }));
  lines.push(JSON.stringify({ type: "result", result: "It changes x to y." }));
  fs.writeFileSync(path.join(bin, "claude"), `#!/bin/sh\nprintf '%s\\n' "$@" > '${kept}.args'\npwd > '${kept}.cwd'\ncat > '${kept}.input'\n${lines.map((line) => `printf '%s\\n' '${line}'\nsleep 0.3\n`).join("")}`, { mode: 0o755 });
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), "-c", "DiffviewOpen HEAD~1", "-c", `lua
      local view
      vim.wait(8000, function() view = require("diffview.lib").get_current_view() return view and view.cur_entry ~= nil end, 50)
      vim.wait(500)
      view.panel:focus()
      -- The box's title as it opens, then the question typed into it and
      -- Enter, in one go, so the box stays in Insert mode throughout.
      local boxed
      local bar
      vim.api.nvim_create_autocmd("WinEnter", { once = true, callback = function() boxed = vim.api.nvim_win_get_config(0).title end })
      vim.api.nvim_create_autocmd("InsertEnter", { once = true, callback = function() bar = vim.wo.winbar end })
      vim.api.nvim_feedkeys(vim.keycode("gaWhat does it change?<CR>"), "xt", false)
      local shown = ""
      vim.wait(8000, function()
        for _, w in ipairs(vim.api.nvim_list_wins()) do
          if vim.api.nvim_win_get_config(w).relative ~= "" then
            local text = table.concat(vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(w), 0, -1, false), "\\n")
            if text:find("x to y", 1, true) then shown = text end
          end
        end
        return shown == "It changes x to y."
      end, 50)
      local answer_bar = ""
      for _, w in ipairs(vim.api.nvim_list_wins()) do
        if vim.api.nvim_win_get_config(w).relative ~= "" then answer_bar = vim.wo[w].winbar end
      end
      io.write(vim.json.encode({ boxed = boxed and boxed[1] and boxed[1][1] or "", shown = shown, bar = bar or "", answer_bar = answer_bar }))`, "-c", "qa!"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(tmp, "ask-cache"), HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "" } },
  );
  const result = JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
  assert.equal(result.boxed, " Ask about this diff ", "ga opens the box");
  assert.equal(result.shown, "It changes x to y.", "the answer streamed into its window");
  assert.match(result.bar, /diff_ask_click@%#SwitchboardButton# Ask %X.*diff_ask_click@%#SwitchboardButton# Cancel %X/, "the box's buttons, each a click");
  assert.match(result.answer_bar, / Close .* Ask again .* Copy /, "and the answer's");
  const args = fs.readFileSync(`${kept}.args`, "utf8").split("\n");
  assert.equal(args[args.indexOf("--tools") + 1], "Read,Grep,Glob", "only the tools that read");
  assert.ok(args.includes("--restricted") && args.includes("--no-session-persistence"));
  assert.equal(fs.realpathSync(fs.readFileSync(`${kept}.cwd`, "utf8").trim()), fs.realpathSync(repo), "in the repository's top");
  const input = fs.readFileSync(`${kept}.input`, "utf8");
  assert.match(input, /<diff>[\s\S]*-x\n\+y[\s\S]*<\/diff>/, "with the diff, read with git since one file tells no story");
  assert.match(input, /<cursor>\nsrc\/a\.js\n<\/cursor>\n\n<question>\nWhat does it change\?\n<\/question>$/, "the file under the list's cursor, then the question");
});

test("a diff to the working tree puts the files only uncommitted in a last chapter, after the story's own, with no why", () => {
  const out = execFileSync("nvim", ["--headless", "-u", "NONE", "-i", "NONE", "--cmd", `set rtp^=${nvimDir}`, "-l", "/dev/stdin"], {
    encoding: "utf8",
    input: `
      local ds = require("diffstory")
      local story = { chapters = { { title = "Change the model", why = "Why.", files = { "a.js" }, touches = { "a.js" } } }, place = { ["a.js"] = 1 }, chapter = { ["a.js"] = 1 }, hunks = {} }
      local both = ds.with_uncommitted(story, { "a.js", "new.js" })
      local alone = ds.with_uncommitted(nil, { "b.js", "c.js" })
      io.write(vim.json.encode({
        both = { titles = vim.tbl_map(function(c) return c.title end, both.chapters), last = both.chapters[2], place = both.place, chapter = both.chapter },
        alone = { titles = vim.tbl_map(function(c) return c.title end, alone.chapters), place = alone.place },
        kept = #story.chapters,
        none = #ds.with_uncommitted(story, { "a.js" }).chapters,
        untold = (function()
          local made = ds.with_uncommitted(nil, { "a.js", "b.js" }, { "a.js" })
          return { titles = vim.tbl_map(function(c) return c.title end, made.chapters), place = made.place, chapter = made.chapter }
        end)(),
      }))
    `,
  });
  const result = JSON.parse(out);
  assert.deepEqual(result.both.titles, ["Change the model", "Not committed yet"]);
  assert.deepEqual(result.both.last, { title: "Not committed yet", why: "", files: ["new.js"], touches: ["a.js", "new.js"] }, "a file in both keeps its committed chapter; the last one's page shows every uncommitted file");
  assert.deepEqual(result.both.place, { "a.js": 1, "new.js": 2 });
  assert.deepEqual(result.both.chapter, { "a.js": 1, "new.js": 2 });
  assert.deepEqual(result.alone, { titles: ["Not committed yet"], place: { "b.js": 1, "c.js": 2 } }, "with nothing committed, that chapter alone");
  assert.equal(result.kept, 1, "the story told is left as it was");
  assert.equal(result.none, 1, "no last chapter when every uncommitted file has one");
  assert.deepEqual(result.untold, { titles: ["Committed", "Not committed yet"], place: { "a.js": 1, "b.js": 2 }, chapter: { "a.js": 1, "b.js": 2 } }, "with no story told of the commits, they go first, so the uncommitted chapter stays last");
});

// M-d's Neovim on a diff from `rev` (with g:diff_uncommitted, as
// pane-in-place.sh opens a branch with uncommitted changes), once its story
// settles: the list's order, the files marked ●, the chapters' headings, each
// bar, and whether a cover came up.
const openUncommitted = ({ repo, bin, home, config, cache, rev, uncommitted = true, env = {} }) => {
  const run = spawnSync(
    "nvim",
    ["--headless", "-u", path.join(nvimDir, "diff.lua"), ...(uncommitted ? ["--cmd", "let g:diff_uncommitted = 1"] : []), "-c", `DiffviewOpen ${rev}`, "-c", `lua
      local view = require("diffview.lib").get_current_view()
      local covered = false
      local function look()
        for _, w in ipairs(vim.api.nvim_list_wins()) do
          if vim.api.nvim_win_get_config(w).relative ~= "" then covered = true end
        end
      end
      vim.wait(8000, function() look() return view.settled and #view.panel:ordered_file_list() > 0 and (view.dirty or not view.uncommitted) end, 10)
      vim.wait(300, function() look() return false end, 10)
      local panel = view.panel
      local marked, headings = {}, {}
      for _, m in ipairs(vim.api.nvim_buf_get_extmarks(panel.bufid, -1, 0, -1, { details = true })) do
        local line = vim.api.nvim_buf_get_lines(panel.bufid, m[2], m[2] + 1, false)[1]
        if m[4].virt_text and m[4].virt_text[1][1] == "●" then table.insert(marked, line) end
        if m[4].virt_lines then table.insert(headings, (m[4].virt_lines[1][1][1] ~= "" and m[4].virt_lines[1] or m[4].virt_lines[2])[1][1]) end
      end
      local bars, links = {}, {}
      for _, entry in ipairs(panel:ordered_file_list()) do
        view:set_file(entry, false)
        vim.wait(1000, function() return view.cur_entry == entry end, 10)
        vim.wait(100)
        for _, w in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
          if vim.wo[w].winbar:find(vim.fs.basename(entry.path), 1, true) then bars[entry.path] = vim.wo[w].winbar end
        end
        -- yl in the working tree's side: what it says.
        local main = view.cur_layout:get_main_win()
        vim.api.nvim_set_current_win(main.id)
        local notify = vim.notify
        vim.notify = function(text) links[entry.path] = text end
        vim.api.nvim_feedkeys("yl", "x", false)
        vim.notify = notify
      end
      io.write(vim.json.encode({ covered = covered, order = vim.tbl_map(function(e) return e.path end, panel:ordered_file_list()), marked = marked, headings = headings, bars = bars, links = links }))`, "-c", "qa!"],
    { cwd: repo, encoding: "utf8", env: { ...process.env, PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: cache, HOME: home, XDG_CONFIG_HOME: config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "", ...env } },
  );
  return JSON.parse(`${run.stdout}${run.stderr}`.trim().split("\n").at(-1));
};
const uncommittedSetup = (name) => {
  const root = path.join(tmp, name);
  const dirs = { repo: path.join(root, "repo"), bin: path.join(root, "bin"), home: path.join(root, "home"), config: path.join(root, "config"), cache: path.join(root, "cache") };
  [path.join(dirs.repo, "src"), dirs.bin, dirs.home, dirs.config].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  fs.symlinkSync(nvimDir, path.join(dirs.config, "nvim"));
  const asked = path.join(root, "asked");
  const answer = JSON.stringify({ chapters: [{ title: "Change the model", why: "The data changes shape.", files: ["src/model.js"] }, { title: "Show it", why: "The page shows it.", files: ["src/ui.js"] }] });
  fs.writeFileSync(path.join(dirs.bin, "claude"), `#!/bin/sh\ncat > '${root}/given'\necho asked >> '${asked}'\nprintf '%s\\n' '${JSON.stringify({ type: "result", result: answer })}'\n`, { mode: 0o755 });
  const git = (...args) => execFileSync("git", ["-C", dirs.repo, "-c", "user.email=t@e", "-c", "user.name=t", ...args]);
  git("init", "-q");
  ["model", "ui", "page"].forEach((file) => fs.writeFileSync(path.join(dirs.repo, `src/${file}.js`), "a\n"));
  git("add", ".");
  git("commit", "-qm", "one");
  const askedCount = () => (fs.existsSync(asked) ? fs.readFileSync(asked, "utf8").trim().split("\n").length : 0);
  return { ...dirs, root, git, askedCount };
};

test("M-d on a branch with nothing committed lists its uncommitted changes, untracked files too, marked, in a chapter of their own, with no cover and no ask", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const setup = uncommittedSetup("nothing-committed");
  fs.writeFileSync(path.join(setup.repo, "src/page.js"), "edited\n");
  fs.writeFileSync(path.join(setup.repo, "src/new.js"), "new\n");
  const result = openUncommitted({ ...setup, rev: "HEAD" });
  assert.deepEqual([...result.order].sort(), ["src/new.js", "src/page.js"], "the edit and the untracked file");
  assert.equal(result.marked.length, 2, "both marked");
  assert.deepEqual(result.headings, ["1/1 Not committed yet"]);
  assert.match(result.bars["src/page.js"], /● uncommitted/, "the bar says so");
  assert.equal(result.covered, false, "no story to wait for under a cover");
  assert.equal(setup.askedCount(), 0, "Haiku isn't asked");
  // With diff_order off, diffview's folder tree: each file marked inside its folder.
  fs.writeFileSync(path.join(setup.bin, "tmux"), "#!/bin/sh\ncase \"$*\" in *@setting_diff_order*) echo off ;; esac\n", { mode: 0o755 });
  const tree = openUncommitted({ ...setup, rev: "HEAD", env: { TMUX: "fake" } });
  assert.deepEqual(tree.marked.map((line) => line.match(/(\w+)\.js/)[1]).sort(), ["new", "page"], "both marked in the tree");
});

test("M-d on a branch with commits and uncommitted changes tells the commits' story, kept from before, with the uncommitted files last and marked", { skip: !ready && "needs lazy.nvim's diffview", timeout: 90_000 }, () => {
  const setup = uncommittedSetup("both");
  fs.writeFileSync(path.join(setup.repo, "src/model.js"), "A\n");
  fs.writeFileSync(path.join(setup.repo, "src/ui.js"), "B\n");
  setup.git("commit", "-qam", "two");
  // The clean branch first, as M-d opens it today (and as the story told ahead does).
  const clean = openUncommitted({ ...setup, rev: "HEAD~1...HEAD", uncommitted: false });
  assert.deepEqual(clean.order, ["src/model.js", "src/ui.js"]);
  assert.deepEqual(clean.marked, [], "nothing marked on a clean branch");
  assert.equal(setup.askedCount(), 1);
  // Clean, to the working tree, as M-d opens it now: the same story, nothing marked, no chapter added.
  const cleanTree = openUncommitted({ ...setup, rev: "HEAD~1" });
  assert.deepEqual(cleanTree.order, ["src/model.js", "src/ui.js"]);
  assert.deepEqual(cleanTree.headings, ["1/2 Change the model", "2/2 Show it"]);
  assert.deepEqual(cleanTree.marked, []);
  assert.equal(setup.askedCount(), 1, "the commits' story reused, not asked again");
  fs.writeFileSync(path.join(setup.repo, "src/ui.js"), "B2\n");
  fs.writeFileSync(path.join(setup.repo, "src/page.js"), "edited\n");
  fs.writeFileSync(path.join(setup.repo, "src/new.js"), "new\n");
  const result = openUncommitted({ ...setup, rev: "HEAD~1" });
  assert.deepEqual(result.order, ["src/model.js", "src/ui.js", "src/new.js", "src/page.js"], "the story's chapters, then the files only uncommitted");
  assert.deepEqual(result.headings, ["1/3 Change the model", "2/3 Show it", "3/3 Not committed yet"]);
  assert.deepEqual(result.marked.map((line) => line.match(/(\w+)\.js/)[1]).sort(), ["new", "page", "ui"], "only the files with uncommitted changes marked, one in both included");
  assert.doesNotMatch(result.bars["src/model.js"], /uncommitted/, "a file changed only in commits shows as before");
  assert.match(result.bars["src/ui.js"], /● uncommitted/);
  assert.match(result.links["src/new.js"], /has uncommitted changes, so this side has no GitHub link yet/, "yl gives no link to a line not committed");
  assert.doesNotMatch(result.links["src/model.js"] ?? "", /uncommitted/, "and goes on as before for a file changed only in commits");
  assert.equal(result.covered, false, "the commits' story is known, so no cover");
  assert.equal(setup.askedCount(), 1, "and Haiku isn't asked again");
});

test("M-d says when there is nothing to show, opens the working tree with the setting on, and the commits alone with it off", { timeout: 60_000 }, () => {
  const root = path.join(tmp, "pane-in-place");
  const tools = path.join(root, "tools");
  const bin = path.join(root, "bin");
  const repo = path.join(root, "repo");
  const origin = path.join(root, "origin.git");
  [tools, bin, path.join(root, "home")].forEach((dir) => fs.mkdirSync(dir, { recursive: true }));
  fs.copyFileSync(path.join(__dirname, "../tmux/pane-in-place.sh"), path.join(tools, "pane-in-place.sh"));
  fs.symlinkSync(path.join(__dirname, "../tmux/diff-base.sh"), path.join(tools, "diff-base.sh"));
  const log = path.join(root, "log");
  fs.writeFileSync(path.join(tools, "say.sh"), `#!/bin/sh\n[ "$1" = wait ] && exit 0\nprintf 'said %s\\n' "$3" >> '${log}'\n`, { mode: 0o755 });
  // A tmux that answers the setting from SETTING and keeps each popup's command.
  fs.writeFileSync(path.join(bin, "tmux"), `#!/bin/sh\ncase "$1" in\n  show) printf '%s\\n' "$SETTING" ;;\n  display-popup) for a; do last=$a; done; case "$*" in *-E*-d*) printf 'opened %s\\n' "$last" >> '${log}' ;; esac ;;\nesac\n`, { mode: 0o755 });
  fs.writeFileSync(path.join(bin, "gh"), "#!/bin/sh\necho main\n", { mode: 0o755 });
  const env = { ...process.env, HOME: path.join(root, "home"), PATH: `${bin}:${process.env.PATH}`, XDG_CACHE_HOME: path.join(root, "cache"), GIT_CONFIG_GLOBAL: "/dev/null", GIT_AUTHOR_NAME: "t", GIT_AUTHOR_EMAIL: "t@e", GIT_COMMITTER_NAME: "t", GIT_COMMITTER_EMAIL: "t@e" };
  const git = (...args) => execFileSync("git", ["-C", repo, ...args], { env, stdio: "ignore" });
  execFileSync("git", ["init", "-q", "--bare", "-b", "main", origin], { env });
  execFileSync("git", ["clone", "-q", origin, repo], { env, stdio: "ignore" });
  fs.writeFileSync(path.join(repo, "a.js"), "a\n");
  git("add", ".");
  git("commit", "-qm", "one");
  git("push", "-q", "origin", "HEAD:main");
  git("checkout", "-q", "-b", "feature");
  const mergeBase = execFileSync("git", ["-C", repo, "rev-parse", "HEAD"], { env, encoding: "utf8" }).trim();
  const press = (setting = "") => {
    fs.rmSync(log, { force: true });
    spawnSync("bash", [path.join(tools, "pane-in-place.sh"), "popup", "diffview", repo, "client"], { env: { ...env, SETTING: setting }, encoding: "utf8", timeout: 30_000 });
    return fs.existsSync(log) ? fs.readFileSync(log, "utf8").trim() : "";
  };
  assert.equal(press(), "said Nothing to show: no commits past origin/main and no uncommitted changes", "a box, and no diff");
  fs.writeFileSync(path.join(repo, "new.js"), "new\n");
  const dirty = press();
  assert.match(dirty, /^opened .*let g:diff_uncommitted = 1.*DiffviewOpen ([0-9a-f]{40})'$/, "an untracked file opens the diff to the working tree");
  assert.equal(dirty.match(/DiffviewOpen ([0-9a-f]{40})/)[1], mergeBase, "from the merge base");
  assert.equal(press("off"), "said Nothing to show: no commits past origin/main", "with the setting off, the commits alone");
  fs.writeFileSync(path.join(repo, "a.js"), "A\n");
  git("commit", "-qam", "two");
  fs.rmSync(path.join(repo, "new.js"));
  assert.match(press(), /^opened .*let g:diff_uncommitted = 1.*DiffviewOpen [0-9a-f]{40}'$/, "a clean branch with commits opens to the working tree too, so later edits come in on a refresh");
  assert.match(press("off"), /^opened .*DiffviewOpen origin\/main\.\.\.HEAD'$/, "with the setting off, its commits alone, as before");
  assert.doesNotMatch(press("off"), /diff_uncommitted/);
});

test("clearing a tab's diff cache clears the file order of the diff M-d opens to the working tree too", { skip: !ready && "needs lazy.nvim's diffview", timeout: 60_000 }, () => {
  const setup = uncommittedSetup("clear-working");
  // Haiku's file order, as ask_order asks for it.
  fs.writeFileSync(path.join(setup.bin, "claude"), `#!/bin/sh\ncat > /dev/null\necho '["src/ui.js", "src/model.js", "src/page.js", "src/new.js"]'\n`, { mode: 0o755 });
  // The diff_story setting off, so the list is in Haiku's order alone.
  fs.writeFileSync(path.join(setup.bin, "tmux"), "#!/bin/sh\ncase \"$*\" in *@setting_diff_story\\ *|*@setting_diff_story) echo off ;; esac\n", { mode: 0o755 });
  fs.writeFileSync(path.join(setup.repo, "src/model.js"), "A\n");
  fs.writeFileSync(path.join(setup.repo, "src/ui.js"), "B\n");
  setup.git("commit", "-qam", "two");
  fs.writeFileSync(path.join(setup.repo, "src/page.js"), "edited\n");
  fs.writeFileSync(path.join(setup.repo, "src/new.js"), "new\n");
  const orders = path.join(setup.cache, "nvim/diff-order");
  const env = { ...process.env, PATH: `${setup.bin}:${process.env.PATH}`, XDG_CACHE_HOME: setup.cache, HOME: setup.home, XDG_CONFIG_HOME: setup.config, XDG_DATA_HOME: process.env.XDG_DATA_HOME ?? path.join(os.homedir(), ".local/share"), TMUX: "fake" };
  spawnSync("nvim", ["--headless", "-u", path.join(nvimDir, "diff.lua"), "--cmd", "let g:diff_uncommitted = 1", "-c", "DiffviewOpen HEAD~1", "-c", `lua vim.wait(8000, function() return #vim.fn.glob("${orders}/*.json", false, true) > 0 end, 20)`, "-c", "qa!"], { cwd: setup.repo, encoding: "utf8", env });
  assert.equal(fs.readdirSync(orders).length, 1, "the working tree's file order kept");
  // As diff-cache-clear.sh opens it: the commits alone.
  const run = spawnSync("nvim", ["--headless", "-u", path.join(nvimDir, "diff.lua"), "--noplugin", "--cmd", "let g:diff_clear = 1", "-c", "DiffviewOpen HEAD~1...HEAD", "-c", "lua vim.defer_fn(function() vim.cmd('cquit 2') end, 20000)"], { cwd: setup.repo, encoding: "utf8", env });
  assert.match(`${run.stdout}${run.stderr}`, /cleared 1/);
  assert.deepEqual(fs.readdirSync(orders), [], "and cleared");
});
