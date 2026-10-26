#!/bin/sh
# Regenerates the README screenshots from the demo workspace: drives
# linear-tui --demo in a detached tmux session, captures the pane and renders
# it with termshot (https://github.com/momiji-rs/termshot).
#
#   scripts/screenshots.sh            # writes docs/images/*.png
#   TERMSHOT=/path/to/termshot scripts/screenshots.sh
set -eu
cd "$(dirname "$0")/.."
termshot=${TERMSHOT:-termshot}
command -v "$termshot" >/dev/null || { echo "termshot not found; set TERMSHOT" >&2; exit 1; }
cargo build --quiet
out=docs/images
session=linear-tui-shot

# shot NAME COLSxROWS KEYS...: start the demo, send each key, capture.
shot() {
	name=$1 size=$2
	shift 2
	tmux kill-session -t "$session" 2>/dev/null || true
	tmux new-session -d -s "$session" -x "${size%x*}" -y "${size#*x}" "target/debug/linear-tui --demo"
	sleep 1
	for key in "$@"; do
		tmux send-keys -t "$session" "$key"
		sleep 0.4
	done
	sleep 0.6
	tmux capture-pane -t "$session" -e -p |
		"$termshot" --lf-newline --size "$size" --cursor none --px 28 --padding 16 - "$out/$name.png"
	tmux kill-session -t "$session"
	echo "$out/$name.png"
}

shot list 132x22 2 j
shot detail 160x46 Enter
shot comments 100x40 Enter G
