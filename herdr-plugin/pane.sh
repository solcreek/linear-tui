#!/bin/sh
# The tui and popup panes: linear-tui with what open.sh passed in
# LINEAR_TUI_BIN, LINEAR_TUI_ARGS, LINEAR_TUI_DEMO and LINEAR_TUI_ISSUE. When
# it cannot start, the reason stays on screen until enter, instead of a pane
# that closes before anyone can read it.

fail() {
    printf '%s\n\nPress enter to close.' "$1"
    read -r _
    exit 1
}

bin=${LINEAR_TUI_BIN:-}
[ -n "$bin" ] || fail "linear-tui: not found in herdr's PATH, ~/.cargo/bin, ~/go/bin,
~/.local/bin or Homebrew's bin. Install it (https://github.com/solcreek/linear-tui),
or set LINEAR_TUI_BIN in herdr's environment."

# shellcheck disable=SC2086 # LINEAR_TUI_ARGS is a list of arguments
"$bin" ${LINEAR_TUI_ARGS:-} ${LINEAR_TUI_DEMO:+--demo} ${LINEAR_TUI_ISSUE:+"$LINEAR_TUI_ISSUE"}
status=$?
[ "$status" -eq 0 ] && exit 0
# Blame linearctl only when linear-tui had to find it and could not: not for
# the demo, a --linearctl given, or any other failure.
if [ -z "${LINEAR_TUI_DEMO:-}" ] && [ -z "${LINEARCTL:-}" ] && ! command -v linearctl >/dev/null 2>&1; then
    case " ${LINEAR_TUI_ARGS:-} " in
    *" --demo "* | *" --linearctl "*) ;;
    *) fail "
linearctl was not found: install it and log in
(https://github.com/solcreek/linear-cli), or set LINEARCTL in herdr's
environment. The demo action needs neither." ;;
    esac
fi
fail ""
