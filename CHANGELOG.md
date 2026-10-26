# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added

- Design document (`docs/design.md`): screens and keybindings, the linearctl
  command behind each screen and how often it runs, the request budget, image
  fetching, caching and sizing, the Markdown approach, the test plan, and what
  linearctl lacks for the TUI.
- `contract` module: checks command lines against `linearctl describe --json`.
  `cargo test` runs it over every linearctl call in the design, against a
  linearctl 0.1.0 snapshot, or the installed binary with `LINEARCTL_LIVE`.
- CI on Linux and macOS: rustfmt, clippy, tests, MSRV 1.88, 90% line coverage.

[Unreleased]: https://github.com/solcreek/linear-tui/commits/main
