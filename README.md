# linear-tui

A terminal UI for Linear, in Rust and ratatui. It holds no API key and never
calls api.linear.app: every read and write is a `linearctl … --json` call, so
anything a person does here an agent can do with the CLI.

Status: design. See [docs/design.md](docs/design.md).

## Tests

    make test       # unit tests, and the design doc checked against linearctl 0.1.0
    make cover      # the same with branch coverage (uses uvx)
    make contract-live   # the design against the linearctl on PATH
