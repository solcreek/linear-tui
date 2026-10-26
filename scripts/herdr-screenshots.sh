#!/bin/sh
# Regenerates the README's herdr screenshots: the plugin in herdr-plugin/ on
# the demo workspace, in a herdr that shares nothing with yours (its own HOME
# and server), recorded on a PTY by examples/pty-record.rs and rendered with
# termshot, as scripts/screenshots.sh.
#
#   scripts/herdr-screenshots.sh      # writes docs/images/herdr-*.png
#   TERMSHOT=/path/to/termshot scripts/herdr-screenshots.sh
set -eu
cd "$(dirname "$0")/.."
root=$(pwd)
termshot=${TERMSHOT:-termshot}
command -v "$termshot" >/dev/null || { echo "termshot not found; set TERMSHOT" >&2; exit 1; }
command -v herdr >/dev/null || { echo "herdr not found" >&2; exit 1; }
cargo build --quiet --release --bins --example pty-record
out=docs/images
px=28
size=160x44
# Short: herdr's socket lives under it, and a socket path has ~100 bytes.
home=$(mktemp -d /tmp/lt-herdr.XXXX)
# Run from a herdr pane, herdr would follow an inherited socket or session to
# your server, even with HOME changed: drop them, and the XDG paths, so that
# everything is under HOME. (Not set to paths inside it: herdr 0.9.5 with
# XDG_CONFIG_HOME set gives its panes another cell size than the PTY's.)
for v in $(env | sed -n 's/^\(HERDR_[A-Za-z0-9_]*\)=.*/\1/p'); do unset "$v"; done
unset TMUX TERM_PROGRAM TERM_PROGRAM_VERSION \
	XDG_CONFIG_HOME XDG_STATE_HOME XDG_DATA_HOME XDG_CACHE_HOME XDG_RUNTIME_DIR
log=$home/pty.log
trap 'HOME=$home herdr server stop >/dev/null 2>&1 || true; rm -rf "$home"' EXIT

# herdr (by the PTY) and linear-tui (by --cell-size) are told termshot's cell
# size, so the image fits whole rows and herdr's strips of it are exactly one
# termshot row high. Asked inside herdr, linear-tui sometimes gets another.
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

# shot NAME PLACEMENT STEP...: a fresh herdr, `open.sh demo PLACEMENT`, then
# each STEP (a pty-record step line), and the last frame. Give an issue time
# to open before scrolling: scrolled while it loads, its image leaves strips.
shot() {
	name=$1 placement=$2
	shift 2
	HOME=$home herdr server stop >/dev/null 2>&1 || true
	rm -rf "$home/.config/herdr"
	# First run: the welcome, then the agent integrations offer.
	cat >"$home/steps" <<-EOF
		sleep 4
		keys \r
		sleep 0.5
		keys \x1b
		sleep 0.5
		run HOME=$home herdr plugin link $root/herdr-plugin >/dev/null
		keys clear; git --no-pager log --oneline --decorate\r
		sleep 1
		run HOME=$home LINEAR_TUI_BIN=$root/target/release/linear-tui LINEAR_TUI_ARGS="--images kitty --cell-size $cell" sh $root/herdr-plugin/open.sh demo $placement >/dev/null
		sleep 2
	EOF
	printf '%s\n' "$@" 'sleep 2' >>"$home/steps"
	(cd "$repo" && HOME=$home SHELL=/bin/zsh TERM=xterm-kitty \
		"$root/target/release/examples/pty-record" "$log" "$size" "$cell" "$home/steps" -- herdr)
	"$termshot" --size "$size" --cursor none --px $px --padding 16 "$log" "$out/$name.png"
	echo "$out/$name.png"
}

shot herdr-popup popup 'keys 2'
shot herdr-split split 'keys \r' 'sleep 2' 'keys jjjjjjjj' 'sleep 1'
