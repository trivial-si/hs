#!/bin/sh
# Reproducible hs demo with fictional tasks in a throwaway database.
#
# Usage: docs/demo.sh [--no-tui]
#
# Set HS to choose the executable, for example HS=./target/release/hs.
# Everything is written under a new temporary directory that is removed on
# exit, so your default database is never opened.
set -eu

HS=${HS:-hs}
open_tui=yes
case "${1:-}" in
  --no-tui) open_tui=no ;;
  '') ;;
  *) echo "usage: $0 [--no-tui]" >&2; exit 2 ;;
esac

demo_dir=$(mktemp -d "${TMPDIR:-/tmp}/hs-demo.XXXXXX")
trap 'rm -rf "$demo_dir"' EXIT
trap 'exit 130' INT TERM
# Point the default location at the demo directory as a second safeguard.
export XDG_DATA_HOME="$demo_dir/data"
db="$demo_dir/demo.db"

demo() {
  printf '\n$ hs'
  for arg in "$@"; do
    case $arg in
      *[!A-Za-z0-9._/:=-]*) printf " '%s'" "$arg" ;;
      *) printf ' %s' "$arg" ;;
    esac
  done
  printf '\n'
  "$HS" --db "$db" "$@"
}

due=$(date -d '+3 days' +%F 2>/dev/null || date -v+3d +%F)

# Fictional tasks, one per quadrant plus the inbox.
demo task add 'Send the Lumen Bakery menu update' --quadrant do --category Work --tags client --due "$due"
demo task add 'Outline the Kestrel Observatory talk' --quadrant plan --category Work --tags writing
demo task add 'Book the Harbor Street meeting room' --quadrant delegate --category Work
demo task add 'Sort the 2019 photo archive' --quadrant drop --category Home
demo task add 'Read the Orchard Lane field notes'

demo matrix

# A focus session on the first task, started without a watcher.
demo pomo start --task 1 --detach --no-bell
demo pomo status

if [ "$open_tui" = yes ] && [ -t 0 ] && [ -t 1 ]; then
  printf '\nOpening the terminal interface. Press ? for keys, Space to pause or resume focus, q to quit.\n'
  "$HS" --db "$db" tui --no-sound
fi

demo pomo stop
printf '\nDemo finished. Removing %s\n' "$demo_dir"
