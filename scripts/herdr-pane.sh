#!/bin/sh
# A herdr plugin pane: linear-tui with the arguments given, plus any in
# $LINEAR_TUI_ARGS (e.g. "--images kitty", as image auto-detection cannot see
# through herdr to the terminal outside it).
#   scripts/herdr-pane.sh [ARGS...]
# shellcheck disable=SC2086
exec ./bin/linear-tui ${LINEAR_TUI_ARGS:-} "$@"
