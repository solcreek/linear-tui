#!/bin/sh
# A herdr plugin action: opens one of the plugin's panes.
#   scripts/herdr-open.sh ENTRYPOINT [PLACEMENT]
set -eu
# Popups always open over the active pane, splits beside it.
exec "${HERDR_BIN_PATH:-herdr}" plugin pane open --plugin "$HERDR_PLUGIN_ID" --entrypoint "$1" ${2:+--placement "$2"} --focus
