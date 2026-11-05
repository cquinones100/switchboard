#!/usr/bin/env bash
# Prints the fzf options every list in a popup shares, so each looks like
# mission control's boxes (render.js modalBox): no border of its own inside
# the popup's rounded one, the chosen row on the highlight with a yellow ›
# and its text bright and bold, matches and the prompt in colour81, the
# header and counts dim, in the colors of the mode theme.sh last set
# (menu.sh's menu_colors). Each list in a popup adds them to fzf's defaults:
#
#   export FZF_DEFAULT_OPTS="${FZF_DEFAULT_OPTS:+$FZF_DEFAULT_OPTS }$(fzf-style.sh)"
#
# so its own options still win where it gives them.

. "${BASH_SOURCE[0]%/*}/menu.sh"
menu_colors
printf -- '--no-border --pointer=› --color=pointer:220:bold,bg+:%s,fg+:%s:bold,hl:81,hl+:81:bold,prompt:81,header:%s,info:%s' \
  "$MENU_HIGHLIGHT" "$MENU_BRIGHT" "$MENU_DIM" "$MENU_DIM"
