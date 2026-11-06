#!/usr/bin/env bash
# Clears what the PR and ticket image views (tmux/pr-image.js) keep: the
# pictures, stylesheet and scripts their pages load, the pages drawn and the
# text last read. M-p's "Clear the PR and ticket image cache" runs it once
# confirm.sh has had a yes; it runs in that popup, so it says so after (-a).

cache="${XDG_CACHE_HOME:-$HOME/.cache}/switchboard"
rm -rf "$cache/assets" "$cache/pr-image"
"${0%/*}/say.sh" -a "The PR and ticket image cache is cleared."
