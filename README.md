# linear-tui

[![CI](https://github.com/solcreek/linear-tui/actions/workflows/ci.yml/badge.svg?branch=main)](https://github.com/solcreek/linear-tui/actions/workflows/ci.yml)
[![npm](https://img.shields.io/npm/v/linear-tui?logo=npm)](https://www.npmjs.com/package/linear-tui)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue)](LICENSE)
[![MSRV 1.90](https://img.shields.io/badge/rustc-1.90%2B-orange?logo=rust)](Cargo.toml)
[![Built with Ratatui](https://img.shields.io/badge/built%20with-Ratatui-000?logo=ratatui)](https://ratatui.rs)
[![Data: linearctl](https://img.shields.io/badge/data-linearctl-5e6ad2?logo=linear)](https://github.com/solcreek/linear-cli)

A terminal UI for [Linear](https://linear.app), in Rust and Ratatui. It holds no
API key and never calls api.linear.app: every read and write is a
[`linearctl`](https://github.com/solcreek/linear-cli) `--json` call, so anything
a person does here an agent can do with the CLI.

![linear-tui showing the issue list beside an open issue: steps to reproduce, a JSON code block, the uploaded screenshot of the checkout error drawn inline, a checklist, a quote and sub-issues](docs/images/detail.png)

<sub>The demo workspace (`--demo`): Northwind, its people and its issues are fictional.
Rendered with [termshot](https://github.com/momiji-rs/termshot) by
`scripts/screenshots.sh`.</sub>

## Status

Early, before the first release: the issue list with views, paging, a filter
and full-text search; the issue detail with Markdown, inline images (kitty,
Sixel, iTerm2 or half blocks), sub-issues, relations, attachments and threaded
comments; and writes: state, priority, assignee, labels and project through
pickers, comments and descriptions in `$EDITOR`, archiving, and a form for new
issues. Every write is a linearctl dry run first, and the confirm dialog shows
the exact command. With linearctl 0.2.0 or later the list updates live
(`linearctl watch`) and calls share a few long-lived linearctl processes
(`serve --stdio`); see
[docs/design.md](docs/design.md) for the whole plan. The npm package
`linear-tui` is a name placeholder until the first release.

| | |
|---|---|
| ![The Active view: priority, identifier, state, title, assignee, coloured labels and how long ago each issue changed](docs/images/list.png) | ![An issue scrolled to its threaded comments](docs/images/comments.png) |
| ![The status picker over the Active view, on Todo for the selected issue](docs/images/edit.png) | ![Creating an issue: the confirm dialog shows the exact linearctl issue create command it will run](docs/images/create.png) |

## Try it

Needs Rust 1.90 or newer. The demo needs no Linear account:

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
`/` filters (`enter` searches every issue), `]`/`[` step through issues in the
detail, `s` `p` `a` `L` `P` set state, priority, assignee, labels, project, `c`
creates (or comments, in an issue), `E` edits the description, `?` lists the
rest. The demo takes no writes, but shows every dry run and confirm dialog.

`linear-tui ENG-142` (or the issue's linear.app URL) opens that issue first.

## herdr

In [herdr](https://herdr.dev), the plugin in `herdr-plugin/` opens linear-tui
over the focused pane, and Ctrl+click on a Linear issue link opens the issue
there instead of in the browser:

```sh
herdr plugin install solcreek/linear-tui/herdr-plugin
```

It needs `linear-tui` and `linearctl` on `PATH` or in `~/.cargo/bin`, `~/go/bin`
or Homebrew's bin; or set `LINEAR_TUI_BIN` and `LINEARCTL` in herdr's environment.

## Tests

All tests are Rust.

    cargo test                              # everything; docs/design.md vs linearctl 0.1.0
    LINEARCTL_LIVE=linearctl cargo test     # the design vs the installed linearctl
    cargo llvm-cov --summary-only           # coverage

With Homebrew's Rust (no rustup `llvm-tools`), point cargo-llvm-cov at the
matching LLVM: `LLVM_COV=$(brew --prefix llvm@22)/bin/llvm-cov`
`LLVM_PROFDATA=$(brew --prefix llvm@22)/bin/llvm-profdata`.

Screenshots: `scripts/screenshots.sh` drives `--demo` in tmux, records what it
writes (kitty graphics included) and renders that with termshot (`TERMSHOT=/path/to/termshot` if it is not on `PATH`).
