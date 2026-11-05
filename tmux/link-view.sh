#!/usr/bin/env bash
# A link read in a pane to the right, as the PR (M-v) and ticket (M-i) views
# show theirs, rather than in the browser: a GitHub pull request as the PR
# view lays it out, a Linear or Jira ticket as the ticket view does, a Notion
# page or Google Doc as Claude's connector gives it (lib/link-view.js doc),
# and any other web page as its main text in Markdown (lib/link-view.js),
# drawn by glow and paged with less. A Slack message can't be read without Claude's connector, so its
# pane only offers to open it. M-p's "Open the link being read in the
# browser" opens it, in the Chrome profile its host is given
# (tmux/open-url.sh); q closes the pane.
#
# Usage:
#   link-view.sh pick <pane> [browser|copy]
#                                    M-p: choose a link shown in <pane>, with fzf;
#                                    Enter reads it beside the pane, Ctrl-O opens
#                                    it in the browser; with browser, the other
#                                    way round (M-p's "Open a link from this
#                                    pane in the browser"); with copy, Enter
#                                    copies it (M-p's "Copy a link from this
#                                    pane"), Ctrl-O opens it
#   link-view.sh media <pane>        M-p: choose an image or video <pane> shows (a
#                                    file, from where the pane is, or a link), with
#                                    fzf; Enter opens it in its app (Preview,
#                                    QuickTime) or a link in the browser, Ctrl-O
#                                    shows a file in Finder
#   link-view.sh media-list <pane>   what media lists for <pane>, a line each
#   link-view.sh links <pane>        the links pick lists for <pane>, a line each
#   link-view.sh open <pane> <url>   read <url> in a pane beside <pane>
#   link-view.sh show [<url>]        in that pane: read it, else LINK_VIEW_URL
#   link-view.sh browser <pane>      open the link read in <pane>'s window
#
# The pane carries @link_view and the link in @link_url. It runs under an
# interactive zsh, for the PATH (gh, glow, node) the tmux server lacks, and
# the link reaches it in LINK_VIEW_URL, set with split-window -e, which passes
# it as written, never in the command line, where zsh would read a link's & or
# ( as its own; only a link of nothing but a URL's characters is read.

set -u
here=${0%/*}
repo=${SWITCHBOARD_DIR:-$HOME/switchboard}
# A note found by search.sh comes as a file:// link, read only from the notes.
safe='^(https?|file)://[A-Za-z0-9._~:/?#@!$&()*+,;=%-]+$'

# The links <pane> shows, a "<shown>\t<url>" line each (see pick).
links_of() {
  local pane=$1 notes_dir note links
  # In the notes pane (@notes_pane), the links of the note it shows, top to
  # bottom: Notes hides a link's address behind its text, so its screen has
  # none to find. The note is the one .notes.json says is open, read from
  # the notes directory alone.
  notes_dir=${NOTES_DIR:-$HOME/Notes}
  note=$(jq -r '.last // empty' "$notes_dir/.notes.json" 2>/dev/null)
  if [ "$(tmux display -p -t "$pane" '#{@notes_pane}')" = 1 ] && [ -n "$note" ] && [[ $note != *..* ]] && [ -f "$notes_dir/$note" ]; then
    links=$(perl -CSD -ne '
      while (m{\[([^\]\n]*)\]\((https?://[^)\s]+)\)|(?<![(\[])(https?://[A-Za-z0-9._~:/?#\@!\$&*+,;=%-]+)}g) {
        my ($text, $url) = defined $2 ? ($1, $2) : ("", $3);
        $url =~ s/[.,;:!?]+$// unless defined $2;
        next if $seen{$url}++;
        $text =~ s/\s+/ /g; $text =~ s/^ | $//g;
        $text = substr($text, 0, 57) . "..." if length $text > 60;
        print length $text && $text ne $url ? "$text  \e[38;5;244m$url\e[0m\t$url\n" : "$url\t$url\n";
      }' "$notes_dir/$note")
    [ -n "$links" ] || { echo "No links in ${note%.md}" >&2; return 1; }
  else
    # Every link in what the pane shows and has scrolled past, newest first, a
    # line each: what fzf shows, a tab, the URL. A link written into text (an
    # OSC 8 hyperlink, kept by capture-pane -e) shows its text before its URL.
    # perl, not node: popups have no nvm.
    links=$(tmux capture-pane -p -e -J -S -3000 -t "$pane" | perl -CSD -ne '
      s/\e\[[0-9;:?<=>]*[ -\/]*[@-~]//g;
      while (m{\e\]8;[^;\e\a]*;([^\e\a]*)(?:\e\\|\a)(.*?)\e\]8;[^;\e\a]*;(?:\e\\|\a)|(https?://[A-Za-z0-9._~:/?#\@!\$&()*+,;=%-]+)}g) {
        if (defined $3) { (my $url = $3) =~ s/[.,;:!?)]+$//; push @found, [$url, ""]; next }
        my ($url, $text) = ($1, $2);
        $text =~ s/\e[^\\]*\\?//g; $text =~ s/[\x00-\x1f]+/ /g; $text =~ s/\s+/ /g; $text =~ s/^ | $//g;
        push @found, [$url, $text eq $url ? "" : $text] if $url =~ m{^https?://[A-Za-z0-9._~:/?#\@!\$&()*+,;=%-]+$};
      }
      END {
        my (%text, %seen, @order);
        for (reverse @found) {
          my ($url, $text) = @$_;
          push @order, $url unless $seen{$url}++;
          $text{$url} //= $text if length $text;
        }
        for (@order) {
          my $text = $text{$_} // "";
          $text = substr($text, 0, 57) . "..." if length $text > 60;
          print length $text ? "$text  \e[38;5;244m$_\e[0m\t$_\n" : "$_\t$_\n";
        }
      }')
    [ -n "$links" ] || { echo "No links in this pane" >&2; return 1; }
  fi
  printf '%s\n' "$links"
}

# The images and videos <pane> shows and has scrolled past, a "<shown>\t<path
# or URL>" line each: the files, newest first (a path absolute, ~/, or from
# the pane's directory, only when the file is there), then the links to them
# among the pane's links as the link picker finds them (links_of, a
# hyperlink's target and text included), kept whole, known by the extension
# of their path.
MEDIA='png|jpe?g|gif|webp|heic|svg|bmp|tiff?|mp4|mov|webm|mkv|m4v|avi'
media_of() {
  local pane=$1 dir found
  dir=$(tmux display -p -t "$pane" '#{pane_current_path}')
  found=$(tmux capture-pane -p -J -S -3000 -t "$pane" | MEDIA_DIR=$dir MEDIA=$MEDIA perl -CSD -ne '
    # Text is read as characters (-CSD), so the paths from the environment are
    # too, and a path is checked as the bytes it is on disk.
    use Encode qw(decode_utf8 encode_utf8);
    our ($home, $dir) = map { decode_utf8($_ // "") } @ENV{qw(HOME MEDIA_DIR)};
    my $ext = $ENV{MEDIA};
    while (m{(?:(?<![^\s(\[\{<=])|(?<=[\s(\[\{<=][\x22\x27`])|(?<=^[\x22\x27`]))(?!\x27)((?:~|\.{1,2})?/?[\w.~\@+%/\x27-]*[\w-]\.(?:$ext))(?=\n|\z|[\s,;:!?)\]\}\x22\x27`>]|\.(?:\s|\z))}gi) {
      my $path = $1;
      $path =~ s{^~(?=/)}{$home};
      $path =~ s{^\./}{};
      $path = "$dir/$path" unless $path =~ m{^/};
      push @found, $path if -f encode_utf8($path);
    }
    END {
      my %seen;
      for (reverse @found) {
        next if $seen{$_}++;
        (my $shown = $_) =~ s{^\Q$home\E/}{~/};
        print "$shown\t$_\n";
      }
    }')
  links=$(links_of "$pane" 2>/dev/null | MEDIA=$MEDIA perl -CSD -ne '
    my ($shown, $url) = /^(.*)\t(.*)$/ or next;
    (my $path = $url) =~ s{[?#].*$}{}; $path =~ s{^[a-z]+://[^/]*}{}i;
    print if $path =~ m{\.(?:$ENV{MEDIA})$}i;')
  found=$(printf '%s\n%s\n' "$found" "$links" | grep -v '^$')
  [ -n "$found" ] || { echo "No image or video in this pane" >&2; return 1; }
  printf '%s\n' "$found"
}

case "${1:-}" in
  media-list)
    media_of "${2:?pane}"
    ;;

  media)
    pane=${2:-}
    [ -n "$pane" ] || exit 0
    err="${TMPDIR:-/tmp}/link-view-media.$$"
    found=$(media_of "$pane" 2>"$err") || { "$here/say.sh" -a "$(cat "$err")"; rm -f "$err"; exit 0; }
    rm -f "$err"
    choice=$(printf '%s\n' "$found" | fzf --ansi --delimiter '\t' --with-nth 1 --prompt 'Open: ' --header 'Enter opens it · Ctrl-O shows a file in Finder' --expect ctrl-o --layout reverse) || exit 0
    key=$(printf '%s' "$choice" | sed -n 1p)
    target=$(printf '%s' "$choice" | sed -n 2p | cut -f2)
    [ -n "$target" ] || exit 0
    if [[ $target =~ ^https?:// ]]; then
      [[ $target =~ $safe ]] && exec "$here/open-url.sh" "$target"
      exit 0
    fi
    [ -f "$target" ] || { "$here/say.sh" -a "$target is gone"; exit 0; }
    if command -v open >/dev/null; then
      if [ "$key" = ctrl-o ]; then open -R "$target"; else open "$target"; fi
    elif command -v xdg-open >/dev/null; then
      xdg-open "$([ "$key" = ctrl-o ] && dirname "$target" || printf '%s' "$target")" >/dev/null 2>&1 &
    fi
    ;;

  links)
    links_of "${2:?pane}"
    ;;

  pick)
    pane=${2:-} first=${3:-read}
    [ -n "$pane" ] || exit 0
    links=$(links_of "$pane" 2>"${TMPDIR:-/tmp}/link-view.$$") || { "${0%/*}/say.sh" -a "$(cat "${TMPDIR:-/tmp}/link-view.$$")"; rm -f "${TMPDIR:-/tmp}/link-view.$$"; exit 0; }
    rm -f "${TMPDIR:-/tmp}/link-view.$$"
    # The same list either way; Enter does what was asked for, Ctrl-O the other.
    if [ "$first" = browser ]; then
      prompt='Open: ' header='Enter opens it in the browser · Ctrl-O reads it beside the pane'
    elif [ "$first" = copy ]; then
      prompt='Copy: ' header='Enter copies it · Ctrl-O opens it in the browser'
    else
      prompt='Read: ' header='Enter reads it beside the pane · Ctrl-O opens it in the browser'
    fi
    choice=$(printf '%s\n' "$links" | fzf --ansi --delimiter '\t' --with-nth 1 --prompt "$prompt" --header "$header" --expect ctrl-o --layout reverse) || exit 0
    key=$(printf '%s' "$choice" | sed -n 1p)
    url=$(printf '%s' "$choice" | sed -n 2p | cut -f2)
    [ -n "$url" ] || exit 0
    if [ "$first" = copy ]; then
      [ "$key" = ctrl-o ] && exec "$here/open-url.sh" "$url"
      # The system clipboard (pbcopy on the Mac, wl-copy or xclip on Linux),
      # said as copied only when it took the link; with none of them, tmux's
      # paste buffer, which tmux passes on to the terminal's clipboard only
      # where it can (OSC 52), so it says just that.
      # Said once the Links popup has closed (-a).
      shown=$url
      if command -v pbcopy >/dev/null; then printf '%s' "$url" | pbcopy
      elif command -v wl-copy >/dev/null; then printf '%s' "$url" | wl-copy
      elif command -v xclip >/dev/null; then printf '%s' "$url" | xclip -selection clipboard
      else
        tmux set-buffer -w -- "$url" && "${0%/*}/say.sh" -a "Put $shown in tmux's paste buffer, and the terminal's clipboard if it allows" || "${0%/*}/say.sh" -a "Couldn't copy $shown"
        exit 0
      fi
      if [ $? = 0 ]; then "${0%/*}/say.sh" -a "Copied $shown"; else "${0%/*}/say.sh" -a "Couldn't copy $shown"; fi
      exit 0
    fi
    if { [ "$first" = browser ] && [ "$key" != ctrl-o ]; } || { [ "$first" != browser ] && [ "$key" = ctrl-o ]; }; then exec "$here/open-url.sh" "$url"; fi
    exec "$0" open "$pane" "$url"
    ;;

  open)
    pane=${2:-} url=${3:-}
    [[ -n $pane && $url =~ $safe ]] || { "${0%/*}/say.sh" -a "That isn't a link that can be read here"; exit 0; }
    # The window's link view, if it has one, gives way to this one.
    old=$(tmux list-panes -t "$pane" -F '#{pane_id} #{@link_view}' | awk '$2 == "1" { print $1; exit }')
    [ -n "$old" ] && tmux kill-pane -t "$old"
    dir=$(tmux display -p -t "$pane" '#{pane_current_path}')
    view=$(tmux split-window -h -l 50% -t "$pane" -c "${dir//\#/##}" -P -F '#{pane_id}' -e "LINK_VIEW_URL=$url" "zsh -i -c '$here/link-view.sh show'") || exit 0
    tmux set-option -p -t "$view" @link_view 1
    tmux set-option -p -t "$view" @link_url "$url"
    ;;

  show)
    url=${2:-${LINK_VIEW_URL:-}}
    dim=$'\e[38;5;244m' red=$'\e[31m' reset=$'\e[0m'
    wait_key() {
      printf '\n  %s\n\n  %sPress any key to close.%s' "$1" "$dim" "$reset"
      read -rsn 1
      exit 0
    }
    [[ $url =~ $safe ]] || wait_key "${red}That isn't a link that can be read here.${reset}"
    command -v glow >/dev/null || wait_key "${red}glow isn't installed.${reset} Install it with: brew install glow"
    # A Linear or Jira ticket: the ticket view, which asks the tracker the
    # tracker setting names, so a Jira link is read this way only with Jira
    # as the tracker, and a Linear one with Linear.
    tracker=$(tmux show -gqv @setting_tracker 2>/dev/null)
    # A Jira link is a ticket's page or a board's ?selectedIssue=.
    id=
    if [[ ${tracker:-linear} == linear && $url =~ ^https://linear\.app/[^/]+/issue/([A-Za-z][A-Za-z0-9]*-[0-9]+) ]]; then
      id=${BASH_REMATCH[1]}
    elif [[ $tracker == jira && $url =~ ^https://[A-Za-z0-9-]+\.atlassian\.net/ && $url =~ (/browse/|[?\&]selectedIssue=)([A-Za-z][A-Za-z0-9]*-[0-9]+) ]]; then
      id=${BASH_REMATCH[2]}
    fi
    [ -n "$id" ] && exec "$here/ticket-view.sh" show "$(printf '%s' "$id" | tr 'a-z' 'A-Z')"
    # Slack: only Claude's connector reads it.
    if [[ $url =~ ^https://[^/]*slack\.com/ ]]; then
      printf '\n  A Slack message can'"'"'t be read here, only in Slack.\n\n  %so opens it in the browser · any other key closes%s' "$dim" "$reset"
      read -rsn 1 key
      [ "$key" = o ] && "$here/open-url.sh" "$url"
      exit 0
    fi
    work=$(mktemp -d)
    trap 'rm -rf "$work"' EXIT
    # A note, as search.sh gives one: its Markdown, read from the notes
    # directory and nowhere else.
    if [[ $url =~ ^file:// ]]; then
      encoded=${url#file://}
      file=$(printf '%b' "${encoded//%/\\x}")
      notes=${NOTES_DIR:-$HOME/Notes}
      [[ $file == "$notes"/*.md && $file != *"/."* && -f $file ]] || wait_key "${red}Only a note can be read from a file.${reset}"
      cp "$file" "$work/page.md"
      "$here/pr-view.sh" page page "$work/page.md" "$work"
      exit 0
    fi
    # A GitHub pull request: the PR view's layout.
    if [[ $url =~ ^https://github\.com/[^/]+/[^/]+/pull/[0-9]+ ]]; then
      printf '\n  %sReading the PR...%s' "$dim" "$reset"
      gh pr view "${BASH_REMATCH[0]}" --json number,title,url,state,isDraft,author,baseRefName,headRefName,body > "$work/pr.json" 2>/dev/null ||
        wait_key "${red}GitHub won't show that PR${reset} (gh)."
      "$here/pr-view.sh" page prepare "$work/pr.json" "$work"
      exit 0
    fi
    # A Notion page, Google Doc or Slides deck: through Claude's connectors,
    # unless the link_view_docs setting is off.
    if [[ $url =~ ^https://([a-z0-9-]+\.)?notion\.(so|site|com)/|^https://docs\.google\.com/(document|presentation)/d/ ]] &&
      [ "$(tmux show -gqv @setting_link_view_docs 2>/dev/null)" != off ]; then
      printf '\n  %sReading it through Claude'"'"'s connector...%s' "$dim" "$reset"
      why=$(node "$repo/lib/link-view.js" doc "$url" "$work/page.md" 2>&1 >/dev/null) ||
        wait_key "${red}Couldn't read it:${reset} ${why:-no answer}. M-p's \"Open the link being read in the browser\" opens it there."
      "$here/pr-view.sh" page page "$work/page.md" "$work"
      exit 0
    fi
    # Any other page: its main text, as Markdown.
    printf '\n  %sReading %s...%s' "$dim" "$url" "$reset"
    why=$(node "$repo/lib/link-view.js" page "$url" "$work/page.md" 2>&1 >/dev/null) ||
      wait_key "${red}Couldn't read it:${reset} ${why:-no answer}. M-p's \"Open the link being read in the browser\" opens it there."
    "$here/pr-view.sh" page page "$work/page.md" "$work"
    ;;

  browser)
    pane=${2:-}
    url=$(tmux list-panes -t "$pane" -F '#{@link_view} #{@link_url}' | awk '$1 == "1" { print $2; exit }')
    [ -n "$url" ] || { "${0%/*}/say.sh" "No link is being read in this tab"; exit 0; }
    exec "$here/open-url.sh" "$url"
    ;;

  *)
    echo "Usage: $0 pick <pane> | open <pane> <url> | show <url> | browser <pane>" >&2
    exit 1
    ;;
esac
exit 0
