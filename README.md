# linear-tui

[![CI](https://github.com/solcreek/linear-tui/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/solcreek/linear-tui/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/linear-tui?logo=npm)](https://www.npmjs.com/package/linear-tui)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![MSRV 1.88](https://img.shields.io/badge/rustc-1.88%2B-orange?logo=rust)](Cargo.toml)
[![Built with Ratatui](https://img.shields.io/badge/built%20with-Ratatui-000?logo=ratatui)](https://ratatui.rs)
[![Data: linearctl](https://img.shields.io/badge/data-linearctl-5e6ad2?logo=linear)](https://github.com/solcreek/linear-cli)

A terminal UI for [Linear](https://linear.app), in Rust and Ratatui. It holds no
API key and never calls api.linear.app: every read and write is a
[`linearctl`](https://github.com/solcreek/linear-cli) `--json` call, so anything
a person does here an agent can do with the CLI.

![linear-tui showing the issue list beside an open issue: its Markdown description with steps to reproduce, a JSON code block, a checklist and a quote, then sub-issues, relations and attachments](docs/images/detail.png)

<sub>The demo workspace (`--demo`): Northwind, its people and its issues are fictional.
Rendered with [termshot](https://github.com/momiji-rs/termshot) by
`scripts/screenshots.sh`.</sub>

## Status

Early and read-only: the issue list with views, paging and a filter, and the
issue detail with Markdown, sub-issues, relations, attachments and threaded
comments. Images, search, and writes (create, edit, comment) come next; see
[docs/design.md](docs/design.md) for the whole plan. The npm package
`linear-tui` is a name placeholder until the first release.

| | |
|---|---|
| ![The Active view: priority, identifier, state, title, assignee, coloured labels and how long ago each issue changed](docs/images/list.png) | ![An issue scrolled to its threaded comments](docs/images/comments.png) |

## Try it

Needs Rust 1.88 or newer. The demo needs no Linear account:

```sh
git clone https://github.com/solcreek/linear-tui.git
cd linear-tui
cargo build
target/debug/linear-tui --demo
```

For your workspace, install linearctl and log in (the key stays with linearctl):

```sh
go install github.com/solcreek/linear-cli/cmd/linearctl@latest   # or: npm i -g @solcreek/linear-cli
pbpaste | linearctl auth login --yes
target/debug/linear-tui
```

Keys: `j`/`k` move, `enter` opens, `esc` goes back, `tab` or `1`–`4` switch views,
`/` filters, `]`/`[` step through issues in the detail, `O` opens in the browser,
`?` lists the rest.

## Tests

All tests are Rust.

    cargo test                              # everything; docs/design.md vs linearctl 0.1.0
    LINEARCTL_LIVE=linearctl cargo test     # the design vs the installed linearctl
    cargo llvm-cov --summary-only           # coverage

With Homebrew's Rust (no rustup `llvm-tools`), point cargo-llvm-cov at the
matching LLVM: `LLVM_COV=$(brew --prefix llvm@22)/bin/llvm-cov`
`LLVM_PROFDATA=$(brew --prefix llvm@22)/bin/llvm-profdata`.

Screenshots: `scripts/screenshots.sh` drives `--demo` in tmux and renders the
panes with termshot (`TERMSHOT=/path/to/termshot` if it is not on `PATH`).
