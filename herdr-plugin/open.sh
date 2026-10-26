#!/bin/sh
# Opens linear-tui in a herdr pane: `open.sh` for the issue list, `open.sh link`
# for the Linear issue URL that was Ctrl+clicked.
set -eu

mode=${1:-list}
H=${HERDR_BIN_PATH:-herdr}

# herdr starts plugin commands with a short PATH: look where linear-tui and
# linearctl are usually installed.
PATH="$HOME/.cargo/bin:$HOME/go/bin:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"

bin=${LINEAR_TUI_BIN:-$(command -v linear-tui || true)}
if [ -z "$bin" ]; then
    echo "linear-tui: not found on PATH; set LINEAR_TUI_BIN" >&2
    exit 1
fi
ctl=${LINEARCTL:-$(command -v linearctl || true)}

set -- --plugin "${HERDR_PLUGIN_ID:-solcreek.linear-tui}" --entrypoint tui --focus \
    --env "LINEAR_TUI_BIN=$bin"
if [ -n "$ctl" ]; then
    set -- "$@" --env "LINEARCTL=$ctl"
fi
# No --workspace or --target-pane: herdr opens an overlay only over the
# active pane, and refuses one that names a target.
if [ "$mode" = link ]; then
    url=${HERDR_PLUGIN_CLICKED_URL:-}
    if [ -z "$url" ]; then
        echo "linear-tui: no clicked URL" >&2
        exit 1
    fi
    set -- "$@" --env "LINEAR_TUI_ISSUE=$url"
fi

exec "$H" plugin pane open "$@"
