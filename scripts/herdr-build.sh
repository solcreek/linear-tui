#!/bin/sh
# herdr runs this on `herdr plugin install`: builds linear-tui and the demo's
# fake-linearctl into ./bin, where herdr-plugin.toml expects them.
set -eu
cd "$(dirname "$0")/.."
command -v cargo >/dev/null || {
	echo "linear-tui needs Rust 1.90 or later to build: https://rustup.rs" >&2
	exit 1
}
cargo build --release --locked --bins
mkdir -p bin
cp target/release/linear-tui target/release/fake-linearctl bin/
