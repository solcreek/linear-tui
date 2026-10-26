# linear-tui

A terminal UI for Linear, in Rust and ratatui. It holds no API key and never
calls api.linear.app: every read and write is a `linearctl … --json` call, so
anything a person does here an agent can do with the CLI.

Status: design. See [docs/design.md](docs/design.md).

## Tests

All tests are Rust.

    cargo test                              # unit tests; docs/design.md vs linearctl 0.1.0
    LINEARCTL_LIVE=linearctl cargo test     # the design vs the installed linearctl
    cargo llvm-cov --summary-only           # coverage

With Homebrew's Rust (no rustup `llvm-tools`), point cargo-llvm-cov at the
matching LLVM: `LLVM_COV=$(brew --prefix llvm@22)/bin/llvm-cov`
`LLVM_PROFDATA=$(brew --prefix llvm@22)/bin/llvm-profdata`.
