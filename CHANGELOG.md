# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.0.1] - 2026-10-26

Reserves the npm name; nothing to run yet.

### Added

- Design document (`docs/design.md`): screens and keybindings, the linearctl
  command behind each screen and how often it runs, the request budget, image
  fetching, caching and sizing, the Markdown approach, the test plan, and what
  linearctl lacks for the TUI.
- `contract` module: checks command lines against `linearctl describe --json`.
  `cargo test` runs it over every linearctl call in the design, against a
  linearctl 0.1.0 snapshot, or the installed binary with `LINEARCTL_LIVE`.
- Demo workspace (`demo/workspace.json`): a fictional team, Northwind, with 17
  English issues, comment threads, sub-issues and relations.
- `fake-linearctl`: answers linearctl's read commands from the demo workspace
  in linearctl's JSON shapes and exit codes, and refuses writes. Used by the
  demo and by tests until linearctl has a demo mode.
- npm package `linear-tui` 0.0.1: a placeholder whose command says the TUI is
  not released yet and exits 1.
- CI on Linux and macOS: rustfmt, clippy, tests, MSRV 1.88, 90% line coverage.

[Unreleased]: https://github.com/solcreek/linear-tui/compare/v0.0.1...HEAD
[0.0.1]: https://github.com/solcreek/linear-tui/releases/tag/v0.0.1
