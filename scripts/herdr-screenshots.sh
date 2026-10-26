#!/bin/sh
# Regenerates the README's herdr screenshots: the plugin's demo in a herdr
# that shares nothing with yours (its own HOME, server and tmux), driven in a
# detached tmux session and rendered with termshot, as scripts/screenshots.sh.
#
#   scripts/herdr-screenshots.sh      # writes docs/images/herdr-*.png
#   TERMSHOT=/path/to/termshot scripts/herdr-screenshots.sh
set -eu
cd "$(dirname "$0")/.."
root=$(pwd)
termshot=${TERMSHOT:-termshot}
command -v "$termshot" >/dev/null || { echo "termshot not found; set TERMSHOT" >&2; exit 1; }
command -v herdr >/dev/null || { echo "herdr not found" >&2; exit 1; }
sh scripts/herdr-build.sh >/dev/null
out=docs/images
px=28
size=160x44
# Short: herdr's socket lives under it, and a socket path has ~100 bytes.
home=$(mktemp -d /tmp/lt-herdr.XXXX)
tmux="tmux -L lt-herdr"
log=$home/pty.log
h() { HOME=$home herdr "$@" >/dev/null; }
trap 'HOME=$home herdr server stop >/dev/null 2>&1 || true; $tmux kill-server 2>/dev/null || true; rm -rf "$home"' EXIT

cell=$(printf x | "$termshot" -v --size 1x1 --px $px - /dev/null 2>&1 | sed -n 's/.*cell \([0-9]*x[0-9]*\).*/\1/p')

# The scene: a project on a branch for the demo's ENG-142, a plain prompt.
printf '%s\n' "PROMPT='%F{cyan}%1~%f %# '" 'unsetopt BEEP' >"$home/.zshrc"
repo=$home/northwind
mkdir -p "$repo"
git -C "$repo" init -q -b main
for m in "Checkout v2 behind a flag" "Coupon field on the payment step" "Log rejected coupon codes"; do
	git -C "$repo" -c user.name=Maya -c user.email=maya@example.com commit -q --allow-empty -m "$m"
done
git -C "$repo" switch -q -c maya/eng-142-coupon-whitespace

# shot NAME ENTRYPOINT KEYS...: a fresh herdr, the plugin pane, each key, the
# last frame.
shot() {
	name=$1 entry=$2
	shift 2
	$tmux kill-server 2>/dev/null || true
	HOME=$home herdr server stop >/dev/null 2>&1 || true
	rm -rf "$home/.config/herdr"
	: >"$log"
	# Unset tmux's variables so herdr talks kitty graphics to termshot, which
	# replays the raw log.
	$tmux new-session -d -s shot -x "${size%x*}" -y "${size#*x}" -c "$repo" \
		"sleep 0.5; exec env -u TMUX -u TERM_PROGRAM -u TERM_PROGRAM_VERSION HOME=$home TERM=xterm-kitty SHELL=/bin/zsh LINEAR_TUI_ARGS='--images kitty --cell-size $cell' herdr"
	$tmux pipe-pane -t shot -O "cat >> '$log'"
	sleep 4
	# First run: the welcome, then the agent integrations offer.
	$tmux send-keys -t shot Enter
	sleep 0.5
	$tmux send-keys -t shot Escape
	sleep 0.5
	h plugin link "$root"
	$tmux send-keys -t shot "clear; git --no-pager log --oneline --decorate" Enter
	sleep 1
	h plugin pane open --plugin linear-tui --entrypoint "$entry" --focus
	sleep 2
	for key in "$@"; do
		$tmux send-keys -t shot "$key"
		sleep 0.3
	done
	sleep 1.5
	$tmux pipe-pane -t shot
	"$termshot" --size "$size" --cursor none --px $px --padding 16 "$log" "$out/$name.png"
	echo "$out/$name.png"
}

shot herdr-popup demo-popup 2
shot herdr-split demo Enter j j j j j j j j
