# linear-tui

A terminal UI for [Linear](https://linear.app), in Rust and Ratatui. It holds no
API key and never calls Linear itself: every read and write is a
[`linearctl`](https://github.com/solcreek/linear-cli) `--json` call, so anything
you do in the TUI an agent can do with the CLI.

**This version (0.0.1) is a placeholder.** The TUI is not released yet; running
`linear-tui` prints where to follow it and exits with status 1. The first release
(0.1.0) will ship native binaries and a demo mode that needs no Linear account:

```sh
npx linear-tui --demo
```

Design, progress and source: https://github.com/solcreek/linear-tui

MIT License.
