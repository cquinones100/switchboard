#!/usr/bin/env bash

# The M-p palette: an fzf list of every custom binding, by its description.
# Picking one presses its key, so the action runs exactly as the binding does.
#
# Usage: palette.sh
#
# The list is read from tmux, not kept here: every binding tmux.conf gives a
# description with `bind -N` shows up, so a new binding needs a note and
# nothing else. The root table holds only bindings from tmux.conf; the prefix
# table also holds tmux's own, which carry notes too, so from it only the keys
# tmux.conf binds with -N are taken. The palette table holds commands with no
# hotkey (see tmux.conf); they are shown with no key.
#
# The key cannot be pressed from inside the popup, which would swallow it. This
# leaves it in @palette_choice, and the M-p binding presses it once the popup
# has closed.
#
# The commands picked last come first, newest first: their keys are kept in
# ~/.local/state/tmux-palette/recent, one per line, the last few only. The
# rest follow alphabetically.
#
# What was typed before each pick is kept too, with the key picked
# (~/.local/state/tmux-palette/queries, "<query>\t<key>", the last 300), and
# a command picked after typing the same words ranks first when they are
# typed again (see --rank). The palette_learn setting turns that off.

set -u
# What it keeps names your branches, tickets and prompts: yours alone.
umask 077

# Orders the rows in <file> for <query>, best first, for fzf to show as is.
# fzf's own scoring rates characters, not words: "pr" scores as well at the
# start of "Previous" as on "PR", and still matches the p and r in "pane
# right". Here each word of the query scores 3 when it is a whole word of the
# description, 2 when it starts one, 1 when it is inside one, and 0 otherwise,
# and a row's score is the sum. Ties keep the alphabetical order. fzf still
# filters and highlights, so loose matches stay in the list, below these.
# Before all that, a command picked after this same query comes first, and
# one picked after a longer query this one starts, as "sha" for "shape", next,
# each tier by how often, so what you reach for is found as you type.
#
# Usage (from fzf's reload): palette.sh --rank <file> <query> [<queries file>]
if [ "${1:-}" = --rank ]; then
  # Through the environment, which awk takes as written: -v would read a
  # backslash in either as an escape.
  RANK_QUERY=${3:-} RANK_PICKS=${4:-} awk -F'\t' '
    BEGIN {
      query = ENVIRON["RANK_QUERY"]; picks = ENVIRON["RANK_PICKS"]
      n = split(tolower(query), terms, " ")
      typed = ""
      for (i = 1; i <= n; i++) typed = typed (i > 1 ? " " : "") terms[i]
      if (typed != "" && picks != "")
        while ((getline line < picks) > 0) {
          tab = index(line, "\t")
          if (!tab) continue
          past = substr(line, 1, tab - 1)
          if (past == typed) same[substr(line, tab + 1)]++
          else if (index(past, typed) == 1) longer[substr(line, tab + 1)]++
        }
    }
    {
      text = tolower($2)
      gsub(/[^a-z0-9]+/, " ", text)
      m = split(text, words, " ")
      score = 0
      for (i = 1; i <= n; i++) {
        best = 0
        for (j = 1; j <= m; j++) {
          if (words[j] == terms[i]) { best = 3; break }
          at = index(words[j], terms[i])
          if (at == 1 && best < 2) best = 2
          else if (at > 1 && best < 1) best = 1
        }
        score += best
      }
      printf "%d\t%d\t%d\t%d\t%s\n", same[$1], longer[$1], score, NR, $0
    }' "$2" | sort -t$'\t' -k1,1nr -k2,2nr -k3,3nr -k4,4n | cut -f5-
  exit 0
fi

# tmux rewrites control characters in its output, tab included, without a
# UTF-8 locale.
export LC_ALL=en_US.UTF-8

prefix=$(tmux show -gv prefix)
conf=~/.config/tmux/tmux.conf

# "<keys>\t<description>", with keys as send-keys -K takes them.
split_note='{ key = $1; $1 = ""; sub(/^ +/, ""); print key "\t" $0 }'

bindings() {
  tmux list-keys -N -T root -P '' | awk "$split_note"

  # list-keys ignores a key argument alongside -N, so the whole prefix table is
  # listed and narrowed to the keys tmux.conf binds there with a note.
  ours=$(grep -oE '^bind -N "[^"]*" [^- ][^ ]*' "$conf" | awk '{ print $NF }' | tr '\n' ' ')
  tmux list-keys -N -T prefix -P '' | awk "$split_note" |
    awk -F'\t' -v prefix="$prefix" -v ours="$ours" '
      BEGIN { n = split(ours, keys, " "); for (i = 1; i <= n; i++) wanted[keys[i]] = 1 }
      $1 in wanted { print prefix " " $1 "\t" $2 }'

  # Pressed after switching to the table, which @palette tells the M-p binding.
  tmux list-keys -N -T palette -P '' 2>/dev/null | awk "$split_note" | awk -F'\t' '{ print "@palette " $1 "\t" $2 }'
}

# tmux spells Shift+letter as the capital letter: Option+Shift+J is M-J and
# Shift+W after the prefix is W. Read that way M-J looks like Option+J, so the
# shown key spells the Shift out (M-S-J, M-b S-W). Only the shown key changes;
# the first field, which is what gets pressed, keeps tmux's own name.
show_key='
  function shown(keys,   n, tokens, i, parts, m, out) {
    if (keys ~ /^@palette /) return ""
    n = split(keys, tokens, " ")
    for (i = 1; i <= n; i++) {
      m = split(tokens[i], parts, "-")
      if (parts[m] ~ /^[A-Z]$/) {
        sub(/[A-Z]$/, "S-" parts[m], tokens[i])
      }
      out = out (i > 1 ? " " : "") tokens[i]
    }
    return out
  }'

# The palette itself is left out: picking it would only reopen it.
# Shown as "<description>  <tags>  <key>", the recent ones first and the rest
# sorted by description, with the tags and the key dimmed. The columns are
# fitted to the popup: a description or tags too long for theirs is cut with
# an ellipsis, so every row's tags and key line up. A binding's tags end its
# description in tmux.conf, as #words ("Open lazygit ... #git #commit"); they
# are matched and ranked as any other word, so a search can name what a
# command is about rather than its wording. The rows go to a file so every
# keystroke can rank them again
# (see --rank); --no-sort keeps that order instead of fzf's own.
# --exact matches each word typed as it is written, not as letters scattered
# through a row ("shape" in "Close the pane"), and each still has to be there.
# --no-hscroll keeps each row at the start of its description: fzf otherwise
# slides a row sideways to show the match, so a tag at its end that matched
# would push the description out of sight.
recent_file=${XDG_STATE_HOME:-$HOME/.local/state}/tmux-palette/recent
RECENT_MAX=6
queries_file=${recent_file%/*}/queries
QUERIES_MAX=300
# fzf's border, pointer, margins and scrollbar take 7 columns; the key gets 10,
# the tags 24, and the description the rest.
columns=$(stty size </dev/tty 2>/dev/null | cut -d' ' -f2)
width=$(( ${columns:-70} - 7 ))
tags_width=24 key_width=10
text_width=$(( width - tags_width - key_width - 2 ))
(( text_width < 30 )) && text_width=30
rows=$(mktemp)
trap 'rm -f "$rows"' EXIT
# Commands tagged #github or #tracker are left out while that setting is off
# (tmux/settings.sh), so the palette offers nothing that would ask it.
IFS='|' read -r github tracker learn < <(tmux display -p '#{@setting_github}|#{@setting_tracker}|#{@setting_palette_learn}')
[ "$learn" = off ] && learned_from="" || learned_from=$queries_file
bindings | awk -F'	' -v github="$github" -v tracker="$tracker" '
    $1 == "M-p" { next }
    github == "off" && $2 ~ / #github( |$)/ { next }
    tracker == "off" && $2 ~ / #tracker( |$)/ { next }
    { print }' | sort -t$'\t' -k2,2 |
  awk -F'\t' -v recent_file="$recent_file" '
    BEGIN { while ((getline key < recent_file) > 0) if (!(key in rank)) rank[key] = ++n }
    { print ($1 in rank ? rank[$1] : n + 1) "\t" NR "\t" $0 }' |
  sort -t$'\t' -k1,1n -k2,2n | cut -f3- |
  awk -F'\t' -v tw="$text_width" -v gw="$tags_width" "$show_key"'
    function cut(s, w) { return length(s) > w ? substr(s, 1, w - 1) "…" : s }
    # awk counts bytes, and the ellipsis is three of them but one column.
    function pad(s, w,   n) { n = length(s) - (index(s, "…") ? 2 : 0); return s sprintf("%" (w - n > 0 ? w - n : 0) "s", "") }
    {
      text = $2; tags = ""
      at = index(text, " #")
      if (at) { tags = substr(text, at + 1); text = substr(text, 1, at - 1) }
      printf "%s\t%s \033[38;5;240m%s \033[38;5;244m%s\033[0m\n", $1, pad(cut(text, tw), tw), pad(cut(tags, gw), gw), shown($1)
    }' > "$rows"

# --print-query gives what was typed on the first line, the pick on the second.
# The paths reach the reload in the environment, so none is ever read as shell.
export PALETTE_SELF=$0 PALETTE_ROWS=$rows PALETTE_LEARNED=$learned_from
picked=$(
  fzf --ansi --delimiter=$'\t' --with-nth=2 --no-sort --no-hscroll --exact --print-query \
    --bind 'change:reload("$PALETTE_SELF" --rank "$PALETTE_ROWS" {q} "$PALETTE_LEARNED")' \
    --prompt='action> ' --layout=reverse --border=rounded --height=100% < "$rows"
)
query=$(head -1 <<<"$picked" | tr '[:upper:]' '[:lower:]' | tr -s ' \t' ' ' | sed 's/^ //; s/ $//')
choice=$(sed -n 2p <<<"$picked" | cut -f1)

if [ -n "$choice" ]; then
  tmux set -g @palette_choice "$choice"
  # A command with a hotkey of its own: once the palette, and anything the
  # command opens, have closed, a tip names the key (tip.sh), once for each.
  tmux run-shell -b -d 1 "~/.config/tmux/tip.sh key $(printf %q "$choice" | sed 's/#/##/g')"
  mkdir -p "${recent_file%/*}"
  { printf '%s\n' "$choice"; grep -vxF -- "$choice" "$recent_file" 2>/dev/null; } | head -n "$RECENT_MAX" > "$recent_file.$$" &&
    mv "$recent_file.$$" "$recent_file"
  if [ -n "$query" ] && [ -n "$learned_from" ]; then
    { tail -n $((QUERIES_MAX - 1)) "$queries_file" 2>/dev/null; printf '%s\t%s\n' "$query" "$choice"; } > "$queries_file.$$" &&
      mv "$queries_file.$$" "$queries_file"
  fi
fi
exit 0
