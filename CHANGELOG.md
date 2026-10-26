# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

- The minimum Rust is 1.90: the image decoders need it.

### Added

- Views from `~/.config/linear-tui/config.toml` (`$XDG_CONFIG_HOME` if set):
  `[[view]]` entries with a `name` and the `issue list` flags as `args`,
  replacing the built-in four; checked against linearctl at start.
- `u` undoes the last write: quick edits, a description, a comment or reply
  (deleted), archiving (unarchived) and a new issue (archived). One level; a
  project set where there was none cannot be undone yet (linearctl cannot
  clear a project), and the notice says so.
- `R` in an issue replies in one of its comment threads (linearctl 0.2.0 or
  later): pick the thread, write in `$EDITOR`, confirm.
- `o` cycles the list's order between updated, created and priority.
- Live updates: with linearctl 0.2.0 or later, the list follows `linearctl
  watch` for the view on screen (`● live` in the header): changed issues update
  in place, new ones appear on top, ones that stop matching go, and the open
  issue reloads when it changes. A watch that stops is retried. With 0.1.0 the
  view reloads every two minutes.
- With linearctl 0.2.0 or later, calls go over a few `linearctl serve --stdio`
  processes instead of one process each.
- `linear-tui`, read-only: the issue list with four views (My issues, Active,
  Triage, Recent), paging, a local filter, and the issue detail beside the
  list on wide terminals (Markdown description, sub-issues, relations,
  attachments, threaded comments). `--demo` browses a fictional workspace
  with no Linear account.
- Images in issue descriptions and comments, drawn inline with the kitty,
  Sixel or iTerm2 protocol, or with half blocks elsewhere. Only uploads on
  uploads.linear.app are fetched, from their signed URLs, with linearctl's
  keyed `file download` when a link has expired; files are cached on disk by
  upload path. `i` turns images off and on; `--images MODE` and
  `--cell-size WxH` override what the terminal reports.
- Search: `Enter` in the `/` filter searches every issue's title, description
  and comments on the server; `Esc` returns to the view. At most 20 searches a
  minute, leaving part of Linear's 30 for agents on the same key.
- Editing, from the list or the open issue: `s` state, `p` priority, `a`
  assignee, `L` labels, `P` project through pickers (type to filter), `A`
  assigns to you, `x` archives. Each runs linearctl's dry run first; quick
  edits apply when it changes something.
- `c` in the list opens a form for a new issue; `c` in an issue writes a
  comment and `E` its description, both in `$EDITOR`. These show the exact
  linearctl command before sending. A new issue's id is made when the form
  opens, so a retry never creates two. With linearctl 0.2.0 a description
  write is refused if the issue changed since it was loaded, and the edited
  text is kept as a draft.
- On start, linear-tui checks `linearctl describe` for every command and flag
  it will use, and refuses to start with a clear message if one is missing.

### Fixed

- Editing a description no longer writes the signed URLs of its uploads back
  into Linear, which would break its images when the signatures expire.

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
