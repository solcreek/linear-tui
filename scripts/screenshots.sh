#!/bin/sh
# Regenerates the README screenshots from the demo workspace: drives
# linear-tui --demo in a detached tmux session and renders the result with
# termshot (https://github.com/momiji-rs/termshot).
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
px=28
log=$(mktemp)
trap 'rm -f "$log"; tmux kill-session -t "$session" 2>/dev/null || true' EXIT

# termshot's cell size at this font size, so images are encoded to match it.
cell=$(printf x | "$termshot" -v --size 1x1 --px $px - /dev/null 2>&1 | sed -n 's/.*cell \([0-9]*x[0-9]*\).*/\1/p')

# start COLSxROWS: a tmux session recording everything the app writes.
start() {
	tmux kill-session -t "$session" 2>/dev/null || true
	tmux new-session -d -s "$session" -x "${1%x*}" -y "${1#*x}"
	tmux send-keys -t "$session" clear Enter
	sleep 0.5
	: >"$log"
	tmux pipe-pane -t "$session" -O "cat >> '$log'"
	# Unset tmux's variables so images are sent as kitty graphics, not
	# wrapped for tmux: termshot replays the raw log.
	tmux send-keys -t "$session" "env -u TMUX -u TERM_PROGRAM -u TERM_PROGRAM_VERSION TERM=xterm-kitty target/debug/linear-tui --demo --images kitty --cell-size $cell" Enter
	sleep 2
}

# shot NAME COLSxROWS KEYS...: start, send each key, render the last frame.
shot() {
	name=$1 size=$2
	shift 2
	start "$size"
	for key in "$@"; do
		tmux send-keys -t "$session" "$key"
		sleep 0.3
	done
	sleep 1
	tmux pipe-pane -t "$session"
	"$termshot" --size "$size" --cursor none --px $px --padding 16 "$log" "$out/$name.png"
	echo "$out/$name.png"
}

shot list 132x22 2 j
shot detail 160x46 Enter j j j j j j j j j j
shot comments 100x40 Enter G
