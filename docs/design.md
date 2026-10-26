# linear-tui — design (draft, 2026-10-26)

A terminal UI for Linear in Rust and ratatui. It gets every byte of Linear data
from `linearctl … --json` and writes only through `linearctl`; it never calls
api.linear.app and never sees the API key. Anything a person does here, an agent
can do with the CLI — and the TUI shows the exact command for each action (see
[Command parity](#command-parity)).

Built against linearctl **0.1.0**. Every `linearctl` invocation in this file is
checked against linearctl 0.3.0's `describe` by `cargo test` (`src/contract.rs`);
the commands under "(proposed)" headings do not exist yet.

Facts below marked *verified* were run against the live API on 2026-10-26.

## Goals and non-goals

Goals: triage and work an issue queue fast from a terminal; read issues with their
images and comments; create, edit and comment; stay well inside the API budget
shared with agents using the same key.

Non-goals for v1: projects/cycles/roadmap screens, documents, multiple workspaces,
attachment upload, offline writes, notifications (no `linearctl` command yet).

## Architecture

```
 ┌───────────── linear-tui (one process) ─────────────┐
 │ UI thread: ratatui draw + crossterm events          │
 │   ↑ Msg                 ↓ Effect                    │
 │ App state (Elm-style update; pure, unit-tested)     │
 │   ↓ Effect                                          │
 │ linearctl calls on threads, over 3 serve processes  │
 │ one watch process; 4 image workers                  │
 │   ctl::Runner ── spawn ──▶ linearctl … --json  ─────┼──▶ api.linear.app
 │   images::Loader ── https GET (signed URL) ─────────┼──▶ uploads.linear.app
 │   image cache ($XDG_CACHE_HOME/linear-tui)          │
 └─────────────────────────────────────────────────────┘
```

Plain threads and channels, no async runtime: every call blocks on linearctl
process or one HTTPS request, and a handful run at a time.

Crates: `ratatui` + `crossterm`, `ratatui-image` + `image` (PNG, JPEG, GIF,
WebP, BMP), `pulldown-cmark`, `unicode-width`, `serde` / `serde_json`, `chrono`,
`ureq` (rustls, HTTPS only), `sha2`, `uuid` (v4), `shlex`.

Modules, each tested as it is written (`cargo llvm-cov`: 97% of lines):

| module | does | tested by |
|---|---|---|
| `contract` | reads `linearctl describe`; checks command lines against it | unit; this doc and every argv the app builds, against the 0.1.0 and 0.3.0 snapshots |
| `ctl` | argv builders; `Runner` spawns with a deadline and maps exit codes; `Exec` | unit; the fake binary with injected faults |
| `model` | serde types for every result; unknown fields ignored, missing ones defaulted | fixtures from the fake and from linearctl 0.1.0 |
| `sanitize` | strips control, C1 and bidi characters from all Linear text | unit |
| `markdown` | Markdown → styled, wrapped lines; links and images collected | unit, incl. CJK width and every demo description |
| `images` | fetch, cache, decode | a local HTTP server; the fake for the keyed fallback |
| `app` (+ `app::write`) | state and `update(msg) -> effects`: list, detail, search, pickers, form, writes | unit, no terminal, against the fake in-process |
| `ui` | draws each screen | ratatui `TestBackend` |
| `editor` | the `$EDITOR` round trip | unit, with `sh` and `true`/`false` as editors |
| `fake` | `fake-linearctl`: the demo workspace in linearctl's shapes, writes, faults | unit and as a binary |

### Talking to linearctl

- Binary: `--linearctl PATH`, else `$LINEARCTL`, else `linearctl` on `PATH`.
- `--demo`: the same linearctl with `LINEARCTL_DEMO=1` when its `describe` lists
  the global `--demo` flag (0.2.0), which answers from a built-in fictional
  workspace (seeded from `demo/workspace.json`) with no key and no network; else
  `fake-linearctl` from beside the binary, which serves the same seed.
- On start, run `linearctl describe --json` (local, ~5 ms, no request). Check that
  every command and flag the TUI builds exists — the same check `cargo test` runs on
  this doc — and refuse to start with one clear message if not. The same result
  turns on optional features when linearctl grows them (`watch`, `serve`; see
  [gaps](#what-linearctl-lacks-proposed)).
- Each call runs on its own thread. With linearctl 0.2.0 or later (`serve` in
  `describe`) it goes to one of up to 3 `linearctl serve --stdio` processes,
  started on first use, which keep their connection to Linear and their key
  (linear-cli measured ~80 ms saved a call): a request is `{"id", "argv"}` on a
  line, the answer `{"id", "exit", "result", "stdout", "stderr"}`. A process that
  dies is started again once; if serve cannot run, or with 0.1.0, each call is a
  process of its own, stdin closed, stdout and stderr drained. Either way the
  deadline is 20 s. `file download` (the image fallback) always gets its own
  process, for its working directory. The environment is inherited unchanged; the TUI
  never sets or reads `LINEAR_API_KEY`.
- Into a pipe the JSON is one compact line. Parse stdout whatever the exit code:
  failures also print `{"error", "usage"}` there (*verified*). The stderr hint
  ("3 shown, more remain; --cursor …") is ignored.
- Every list request carries a generation number, and a result for a replaced
  list is dropped; a detail result for an issue no longer open is only cached.

| exit | TUI reaction |
|---|---|
| 0 ok | apply |
| 1 failed | the status line shows `error` |
| 2 usage | a TUI bug: "internal error (bad linearctl invocation): …" |
| 3 partial | not produced by the commands we use (`api query` only); treated as 1 |
| 4 busy | retry after 200 ms, 400 ms, 800 ms, then 1 |
| 5 auth | the status line shows linearctl's message (run `linearctl auth login --yes` with the key on stdin, then `r`). The TUI never takes the key. Later: a full-screen notice |
| 6 rate-limited | the status line shows `retryAt`. Later: pause reads until then |
| 7 conflict | (0.2.0) a description write refused because the issue changed; the text is kept as a draft |

### Writes

Every write is two steps, matching linearctl's own contract:

1. `--dry-run` with the full argv → show `input` (the exact mutation, names
   resolved to ids) in a confirm dialog. A `changed: false` dry run closes the
   dialog with "nothing to change".
2. `--yes` with the same argv.

`issue create` and `comment create` get an `--id` the TUI generates (UUID v4) when
the form opens, used for the dry run and the write; a retry after a timeout reuses
it and can never duplicate (linearctl returns the existing item, `changed: false`).

`issue update --yes` returns the updated issue (*verified* in source), which
replaces the cached row and detail without a re-fetch; `issue create --yes` and
`comment create --yes` return the new item likewise.

Long text goes in a private temporary file (`--description-file PATH`,
`--body-file PATH`, 0600, removed after), never argv: it stays out of `ps`, has
no length limit, and works with `serve --stdio`, which takes no stdin.

Quick edits (state, priority, assignee, labels, project, assign to me) skip the
confirm dialog: the dry run still runs, its error (if any) is shown, and `--yes`
follows only when it changes something. Later: `u` to undo the last one.

## Screens and keybindings

Global keys: `?` help, `q` back / quit from the list, `Ctrl-c` quit, `:` action
palette, `r` refresh the current screen, `y` copy menu (identifier, URL, branch
name, **the linearctl command** for this view or action), `Tab` next view.

### 1. Issue list (home)

```
 Northwind · My issues      assignee:me state-type:started,unstarted    ⟳ 2m ago
 ─────────────────────────────────────────────────────────────────────────────
 !!! ENG-142  In Progress  Render issue images inline with the Kitty protocol  MP  Feature
  !! ENG-139  Todo         …
 ─────────────────────────────────────────────────────────────────────────────
 25 of 25+ · j/k move · ⏎ open · / filter · f filters · c new · ? help
```

Columns: priority glyph, identifier, state (coloured by state type), title,
assignee initials, labels, updated (relative). Narrow terminals drop labels, then
updated, then assignee.

| key | action |
|---|---|
| `j` `k` / arrows, `g` `G`, `Ctrl-d` `Ctrl-u` | move |
| `Enter` / `l` | open detail (split pane at ≥ 140 columns, full screen otherwise) |
| `/` | filter loaded rows locally as you type; `Enter` in the box runs a server search |
| `f` | filter panel (later) |
| `o` | sort cycle: updated → created → priority (later) |
| `1`…`9`, `Tab` | views (from `config.toml`: later; four built in) |
| `s` `p` `a` `L` `P` | quick edit state / priority / assignee / labels / project of the selected issue |
| `A` | assign to me |
| `c` | create issue |
| `x` | archive (confirm) |
| `O` | open in browser |
| `n` at the bottom | load the next page (`--cursor`); also automatic when scrolling past the end |

**Views** are named filter sets, each the `issue list` flags it stands for. Four
are built in; later, `~/.config/linear-tui/config.toml` will define them:

```toml
[[view]]
name = "My issues"
args = ["--assignee", "me", "--state-type", "unstarted", "--state-type", "started", "--sort", "priority"]

[[view]]
name = "Triage"
args = ["--team", "ENG", "--state-type", "triage"]
```

Defaults when none are configured: *My issues*, *Team active* (default team,
started + unstarted), *Triage*, *Recently updated* (`--updated-since 7d`).

**Filter panel** (`f`): one row per `issue list` flag — team, assignee, state type,
state, labels (all of / any of / none of), project, cycle (current, next, number),
updated since, sort. Pickers are fed by the lookup commands. The panel's footer is
the live command line, e.g.
`linearctl issue list --team ENG --any-label bug --without-label wontfix --sort priority`;
`s` saves it as a view.

Calls:

| when | command |
|---|---|
| open a view | `linearctl issue list <view args> --limit 50 --fields id,identifier,title,priority,state,assignee,labels,team,updatedAt --json` |
| scroll past the end | the same with `--cursor <nextCursor>` |
| refresh timer (below) | the same with `--updated-since <newest updatedAt seen>` and `--limit 250`; rows merge by `id` |
| `r` | the first page again, replacing the list |

### 2. Issue detail

```
 ENG-142  Render issue images inline with the Kitty protocol
 In Progress · High · Maya Patel · Feature · Cycle 24 · ↑ ENG-120
 ─────────────────────────────────────────────────────────────────────────────
 Description (Markdown, with images inline)
 ┌───────────────────────┐
 │  [image]              │
 └───────────────────────┘
 Sub-issues (2)   Relations (1)   Attachments (1)
 Comments (5)
   Jonas Berg · 2d ago
     …
     ↪ reply …
```

| key | action |
|---|---|
| `j` `k`, `Ctrl-d` `Ctrl-u`, `g` `G` | scroll |
| `]` `[` | next / previous issue in the list without leaving the detail |
| `E` | description in `$EDITOR` (`e`, a full edit form: later) |
| `s` `p` `a` `L` `P` `A` `x` | as in the list |
| `c` | comment in `$EDITOR` |
| `Tab` | focus the next link, sub-issue, relation or image; `Enter` follows it (later) |
| `i` | toggle images (off → alt text only) |
| `v` | the focused image full screen (later) |
| `d` | download the focused upload (later) |
| `C` | load all comments when more than shown (later) |
| `O` | open in browser |

Calls:

| when | command |
|---|---|
| open | `linearctl issue show <ID> --comments=50 --signed-urls 300 --json` |
| `C` and more than 50 comments | `linearctl comment list <ID> --limit 0 --json` |
| re-open within 2 min | none: the cached result is shown |
| re-open later, or `r` | `issue show` again; shown stale until it returns |

Comments are threaded by `parent`, oldest first, replies indented under their root.

### 3. Search

Two levels, so typing never spends the 30-searches-a-minute budget:

1. `/` filters the rows already loaded, locally, as you type (title, identifier,
   labels, assignee). Free. `↓` or `↑` leaves the box and keeps the filter.
2. `Enter` in the box runs a server-side full-text search (titles, descriptions,
   comments): `linearctl issue list --query <TEXT> --limit 50 --json`. Results
   open as a temporary *Search* view; `Esc` returns to the previous one.
   Later: a "within view" toggle (`Ctrl-v`) adding the current view's filters.

A client-side token bucket allows 20 searches a minute, leaving 10 for agents on
the same key; above it the box says when the next search is allowed.

### 4. Create

`c` opens a form: team (default from config), title, priority, state, assignee,
labels, project, parent, and the description (`Ctrl-e` for `$EDITOR`). Pickers
use the cached lookups. Submit runs

```
linearctl issue create --team ENG --title "…" --priority high --label bug --description-file PATH --id <uuid> --dry-run --json
linearctl issue create --team ENG --title "…" --priority high --label bug --description-file PATH --id <uuid> --yes --json
```

and opens the new issue's detail.

### 5. Edit

- Quick edits: `s` state picker (states of the issue's team, in position order),
  `p` priority (0–4), `a` assignee (`me`, `none`, or an email), `L` labels
  (multi-select; sends `--add-label` / `--remove-label` for the difference, so a
  label added concurrently by someone else survives).
- `P` project, `A` assign to me, `x` archive (confirmed).
- Later: `e`, a full edit form; only changed fields become flags.
- `E` description: the current text in a temp file (0600) in `$EDITOR`; on save,
  `linearctl issue update <ID> --description-file PATH --dry-run --json`, a confirm
  dialog with the exact command, then `--yes`.

Concurrent edits: with linearctl 0.2.0 (detected from `describe`), the write
carries `--if-updated-at` with the `updatedAt` the detail was loaded with, and
linearctl refuses it with exit 7 if the issue changed since. Then, or when any
description write fails or is cancelled, the edited text is kept as a draft, and
the next `E` opens it.

### 6. Comment

`c` in the detail opens `$EDITOR`; on save,
`linearctl comment create <ID> --body-file PATH --id <uuid> --dry-run --json`, a
confirm dialog, then `--yes`. The new comment is appended from the result.
Later: replies in a thread, with linearctl 0.2.0's `comment create --parent`.

### 7. Lookups, auth and status

Loaded lazily, the first time a picker or the filter panel needs them, then cached
on disk for 24 h (and refreshed by `r` inside a picker):

| data | command |
|---|---|
| who am I, rate-limit headroom | `linearctl auth status --json` (at start, then every 10 min) |
| teams | `linearctl team list --limit 0 --json` |
| states of a team | `linearctl state list --team <KEY> --limit 0 --json` |
| labels | `linearctl label list --team <KEY> --limit 0 --json` |
| users | `linearctl user list --active --limit 0 --json` |
| projects | `linearctl project list --team <KEY> --status planned --status started --limit 0 --json` |
| cycles | `linearctl cycle list --team <KEY> --limit 0 --json` |

The status bar shows the workspace, the view's age, in-flight calls, and the
remaining request budget from the last `auth status` once it drops under 20%.

### Command parity

Every screen and every confirm dialog can copy (`y`) the linearctl command it
corresponds to, with `--json`, ready to paste into a shell or hand to an agent.
The argv is the one the TUI really ran, so it is always valid.

## Request budget

linearctl makes 1 request per read and 2 per write (resolve, then mutate); a dry
run is 1 (it resolves). The key's budget is 2,500 requests an hour, shared with any
agent using it, and `--query` search is capped at 30 a minute. Measured cost: ~150 ms
per request, ~5 ms of it linearctl (docs/performance.md in linear-cli).

Refresh policy — no tight polling:

- With linearctl 0.2.0 or later (`watch` in `describe`), each loaded view starts
  one `watch` with the view's filters (not `--sort`, which watch lacks), the
  list's `--fields`, and `--since` the time the list was loaded, so changes made
  in between are replayed. Its lines, one JSON object each, keyed by `event`:
  - `ready {mode, since, known, everything}`: up to date; the header says
    `● live`, or `● live · polling` when linearctl polls;
  - `upsert {issue}`: a list row; it replaces the row with that `id` or goes on
    top, the selection stays on its issue, and if it is the open issue the
    detail runs `issue show` again (watch does not report comments);
  - `remove {id, identifier, reason}`: the row goes; an unknown id is ignored;
  - `heartbeat {at, mode, rateLimit}`: liveness; no line for 75 s (heartbeats
    come every 30 s) restarts the watch;
  - `gap {from, to, reason, mode}`: linearctl replays the gap itself; the status
    line says it reconnected, or fell back to polling.
  A watch that ends (exit 5, 6, 1, with a last `{error}` line) is shown in the
  status line and started again after 30 s. A search has no watch. Cost: 1–2
  requests at start for a filtered view, then at most one check every 2 s while
  issues change, plus 2 per reconcile (300 s).
- Without `watch` (0.1.0), the view is reloaded every **120 s** while nothing is
  loading and no dialog is open. Later: only while the terminal has focus, and
  `--updated-since` deltas instead of whole pages.
- Details are cached until written, reloaded (`r`) or changed per `watch`;
  nothing is prefetched.
- Lookups: once per run.

A busy hour of use:

| what | requests |
|---|---|
| start: `auth status`, `team list`, first page | 3 |
| refresh timer, 30 × delta + 6 × full reload | 36 |
| 60 issues opened | 60 |
| 15 server searches | 15 |
| 10 writes (dry run + write) | 30 |
| lookups for 2 teams, `auth status` × 6 | 16 |
| **total** | **~160 (6% of the hour)** |

## Images

### Where they come from

Images live in descriptions and comment bodies as Markdown,
`![name.jpg](https://uploads.linear.app/<org>/<uuid>/<uuid>?signature=…)`. Other
uploads (video, PDFs) are plain links to the same host (*verified*). The detail
view calls `issue show … --signed-urls 300`, which signs those URLs for 300 s.

Verified on 2026-10-26 against a real upload:

- the signed URL returns the file with no key (`200`, `content-type: image/jpeg`);
  the same URL without `signature` returns `401`;
- the signature is a JWT whose payload is `{path, iat, exp}`; the path is the
  stable identity of the file, the signature changes on every `issue show`;
- the response carries `cache-control: public, max-age=31536000` and an `etag`:
  the bytes behind a path never change.

### Fetching

- Only `https://uploads.linear.app/...` is fetched automatically. Images on other
  hosts are shown as alt text plus link, fetched only when the user focuses one and
  presses `Enter` (a tracking pixel in someone's description should not learn the
  reader's IP on open).
- `reqwest` with rustls; no redirects off the host; 15 s timeout; 20 MB cap;
  4 fetches at a time, ordered by distance from the viewport.
- The body is sniffed (`image::guess_format`) before decoding; anything that is not
  PNG, JPEG, GIF (first frame), WebP or BMP is treated as a non-image upload.
- `401`/`403` means the signature expired (the issue stayed open > 300 s before the
  image scrolled into view): run `issue show` once more for fresh URLs. If that
  still fails, fall back to the keyed download, run with its working directory set
  to the cache staging dir (it only writes inside the current directory):
  `linearctl file download <URL without signature> --output <name> --yes --json`.

### Cache

- Key: SHA-256 of the URL **path** (signature and query stripped). Because the bytes
  behind a path never change, a cached file is never revalidated.
- Disk: `$XDG_CACHE_HOME/linear-tui/<org urlKey>/images/`, original bytes, directory
  0700, files 0600. LRU by access time, 256 MB default (`[images] cache_mb`).
- Memory: decoded images downscaled to the largest size the pane can show,
  LRU 64 MB; encoded protocol state kept per (image, area) so scrolling does not
  re-encode.

### Sizing and drawing

- `ratatui-image`'s `Picker` is created once at start from a terminal query, giving
  the protocol (Kitty, Sixel, iTerm2, else halfblocks) and the cell size in pixels.
  Config `[images] protocol = "auto" | "kitty" | "sixel" | "iterm2" | "halfblocks" | "off"`
  overrides it; inside tmux without `allow-passthrough on`, auto picks halfblocks.
- Box width = min(pane width − 2, image width ÷ cell width); height from the aspect
  ratio, capped at 60% of the pane height (and 30 rows). Never upscaled: a 40 px
  icon stays small.
- Dimensions are read from the header as soon as bytes arrive; until then the
  image is a one-line placeholder `▢ name.jpg (loading…)`, so layout shifts at most
  once per image.
- Decoding, downscaling and protocol encoding run on a blocking thread
  (`image` with limits: 8192 × 8192 px, 256 MB allocation), never on the UI thread.
- An image is drawn only when its whole box is in the viewport; a partly visible
  one shows its placeholder frame. `v` shows one full screen. Cropping partly
  visible images is a later improvement.
- Non-image uploads render as `↗ screen-recording.mov` links: `d` downloads one to
  `~/Downloads` (keyed `file download` with that directory as working directory),
  `Enter` opens it in the browser.

## Markdown

The known gap in the ratatui ecosystem. Decision: **`pulldown-cmark` plus our own
renderer**, not an off-the-shelf widget.

- Why not `tui-markdown` (or similar): they produce a `Text` and have nowhere to put
  an image, which must be a separate widget at a known position; link focus and
  numbering, CJK wrapping and threaded comments also need control of layout.
- The renderer turns Markdown (CommonMark + GFM tables, strikethrough, task lists)
  into `Vec<Block>`: `Text(lines)`, `Image{url, alt}`, `Code{lang, lines}`,
  `Table`, `Rule`, `Quote(Vec<Block>)`. The detail view lays the blocks out for the
  pane width and draws images over their reserved rows.
- Styling: headings bold (H1/H2 underlined), emphasis/strong/strike as styles,
  inline code reversed-dim, code blocks in a box (no highlighting in v1; `syntect`
  behind a feature later), quotes with a `│` gutter, task lists `☐`/`☑`.
- Width: `unicode-width` for every cell count, `textwrap` for wrapping, so Chinese
  and Japanese text (common in real workspaces) wraps correctly.
- Links: shown underlined with a superscript number; `Tab` focuses them. Linear
  issue URLs render as the identifier (`ENG-123`) and open in the TUI.
- **Untrusted text**: titles, descriptions, comment bodies, names and labels are
  written by other people. Every string from Linear passes through one sanitiser
  before it reaches a buffer: C0 controls except `\n` and `\t`, DEL, C1 controls
  and bidi overrides are removed, so no escape sequence from Linear reaches the
  terminal. The TUI never executes or acts on that text.

## Caching of JSON results

- In memory: issue details by identifier (dropped when written or on `r`), and
  lookups (teams, states, labels, users, projects) once per run.
- Later, on disk for instant start: the last first page of each view and the
  lookups, in `$XDG_CACHE_HOME/linear-tui/`, 0600, shown at once marked stale and
  revalidated.
- `--fields` keeps list payloads small; details always fetch all fields.

## herdr plugin

[herdr](https://herdr.dev) is a terminal workspace manager for coding agents. Its
plugins are a directory with a `herdr-plugin.toml` and the commands it launches:
panes (terminal programs, overlay/split/tab/zoomed), actions (bindable to keys),
event hooks, startup hooks and link handlers (Ctrl+click on a matching URL runs
an action instead of the browser). A plugin talks to herdr through the `herdr`
CLI (`$HERDR_BIN_PATH`).

The plugin lives here, in `herdr-plugin/`, not in linear-cli:

- What a plugin can show is a terminal pane; the TUI is that pane. A linearctl
  plugin could only print JSON into a popup, or start linear-tui, turning the
  dependency around (linear-tui depends on linearctl, never the reverse).
- linearctl stays the neutral layer people and agents share, tied to no
  multiplexer. Anything the plugin needs from it must make sense without herdr
  (say, resolving a Linear URL to an identifier) and goes in the
  [list](#what-linearctl-lacks-proposed) like any other ask.
- Every byte of Linear data still comes from linearctl: the plugin's scripts
  start linear-tui, and linear-tui runs linearctl.

Install: `herdr plugin install solcreek/linear-tui/herdr-plugin`, or
`herdr plugin link herdr-plugin` from a checkout while developing.

### Stage 1: a pane and a link handler *(done)*

- `linear-tui ISSUE` opens that issue first; ISSUE is an identifier (`ENG-142`)
  or its `linear.app/<workspace>/issue/ENG-142/…` URL. Anything else is a usage
  error, so a URL that is not an issue never starts the TUI. A URL's issue opens
  once `auth status` shows its workspace is the one linearctl is logged in to;
  another workspace's ENG-142 is a different issue, so it is refused with a
  status line naming both.
- Pane `tui` runs linear-tui; actions `open` (the issue list) and `open-link`
  open it as an overlay over the focused pane, so `esc`/`q` returns to it.
- Link handler `issue` sends Ctrl+clicks on Linear issue URLs to `open-link`,
  which opens a fresh pane on that issue (`HERDR_PLUGIN_CLICKED_URL`).
- herdr starts plugin commands with a short PATH, so `open.sh` looks in the
  usual install directories (`~/.cargo/bin`, `~/go/bin`, Homebrew, `~/.local/bin`)
  and passes the resolved `linear-tui` and `linearctl` to the pane as
  `LINEAR_TUI_BIN` and `LINEARCTL`. Both can be set outright in herdr's
  environment.

### Stage 2: one pane per workspace

A second click opens a second pane. Instead: `open-link` focuses the
workspace's linear-tui pane if it has one and asks it to open the issue. The
TUI needs a way to be asked: it reads open requests from a file in its state
directory (`$XDG_STATE_HOME/linear-tui/<pane>/open`), checked each tick, keyed
by `HERDR_PANE_ID` so two panes do not take each other's requests. A startup
hook reopens linear-tui in the panes it ran in before a herdr restart.

### Stage 3: what agents can see

- linear-tui writes the issue on screen (identifier, URL, title) to
  `$XDG_STATE_HOME/linear-tui/<pane>/focus.json`; an agent beside it reads that
  instead of being told, then uses linearctl for everything else.
- A key that sends a note about the open issue to the adjacent agent pane
  (`herdr pane send-text`), so "look at ENG-142" is one keystroke.
- Issue rows marked with the agent working on them, joined by the issue's
  branch name (Linear's `eng-142-…`) against the worktrees herdr reports on
  `worktree.*` and `pane.agent_*` events.

## Testing

Tests ship with each change, in Rust only; CI fails below 90% line coverage
(`cargo llvm-cov`; 97% now, the uncovered part is mostly the terminal loop).

- **Contract**: the describe snapshots of linearctl 0.1.0 and 0.3.0
  (`tests/fixtures/`) are checked against every argv the app builds and against
  this doc. `LINEARCTL_LIVE=linearctl cargo test` checks the installed one.
- **Fake**: `fake-linearctl` serves `demo/workspace.json` in linearctl's shapes,
  takes writes (kept in `FAKE_LINEARCTL_STATE`), and injects faults
  (`FAKE_LINEARCTL_FAULT`: any exit code, sleeps, garbage, crashes, busy once).
- **Model**: the fake's answers, and rows captured from linearctl 0.1.0 (no
  nested colours).
- **Runner**: the fake binary for each exit code, timeouts, busy retries, crashes.
- **App**: `update` is pure; key sequences in, state and effects out, run against
  the fake in-process (`fake::InProcess`), writes included.
- **UI**: `TestBackend` at 80–160 columns; images drawn with half blocks.
- **Markdown**: CJK, nested lists, tables wider than the pane, hostile text, and
  every demo description at three widths.
- **Images**: a local HTTP server for 200/401/403/302/500/oversized answers; the
  fake for the keyed fallback.
- **By hand**: `scripts/screenshots.sh` renders the demo with termshot, kitty
  graphics included.

## Milestones

1. Skeleton: runner, describe check, issue list with views, detail. *Done.*
2. Markdown renderer and comments. *Done.*
3. Images. *Done.*
4. Search *(done)*; filter panel, sort, configured views.
5. Writes: quick edits, create, comment, description, archive *(done)*; the full
   edit form, undo, thread replies.
6. Live updates with `watch` and calls over `serve --stdio`. *Done.*
7. herdr plugin: a pane and a link handler *(done)*; one pane per workspace;
   what agents can see. See [herdr plugin](#herdr-plugin).
8. Release: per-platform binaries on npm as `linear-tui`, like linearctl's.

## What linearctl lacks (proposed)

Sent to the linear-cli owner as a list; the TUI works around each until it lands.
Ordered by how much the TUI needs it.

1. **`linearctl watch --json` (NDJSON)** taking `issue list`'s filters, emitting
   `{"type":"upsert","issue":{…}}` and `{"type":"remove","id":…}` when an issue
   stops matching. `--updated-since` already gives upserts; the TUI cannot see
   removals without a full reload. One watcher per key also beats each consumer
   polling. *Accepted*: linear-cli will build it on Linear's (undocumented)
   GraphQL subscriptions with a polling fallback, re-checking the filter for each
   changed issue to emit removals. *Landed* on linear-cli main (`ac99981`); the
   shape is under [Request budget](#request-budget).
2. **`linearctl serve --stdio`**: one long-lived process taking NDJSON
   `{"id":…,"argv":[…]}` and answering `{"id":…,"exit":N,"result":{…}}` with the
   same JSON as the CLI. Saves TCP + TLS (~40 ms of each ~150 ms call), the
   process start (4 ms) and the Keychain read (~10 ms) per call. Argv-shaped, so
   the CLI stays the contract.
3. **Colours and ids on nested objects**: `issue list` / `issue show` give
   `state {name, type}` and `labels [{name}]` only (*verified*). Colours exist only
   in `state list` / `label list`, and names are not unique across teams ("Done" in
   every team; label names per team), so the TUI cannot join reliably. Ask:
   `state {id, name, type, color}`, `labels [{id, name, color}]`.
4. **Rate-limit headroom per call**: today only `auth status` reports it, costing a
   request. An opt-in (`--meta`, or `LINEARCTL_META=1`) adding
   `meta {requests {remaining, resetAt}, complexity {…}}` from the response
   headers would let the TUI slow down before exit 6.
5. **Precondition on update**: `issue update --if-updated-at TIME` failing (its own
   exit code or a stable `error` prefix) when the issue changed since, so editing a
   description cannot silently overwrite someone else's edit.
6. **`comment create --parent COMMENT`** for replies in a thread. Output already
   has `comments[].parent`; input cannot set it.
7. **`issue list --priority P`** (repeatable, any of), so a priority filter is
   server-side instead of over loaded rows only.
8. **Batch read**: `issue show A B C` or `issue list --id ID…`, to refresh the
   rows on screen in one request after a reconnect or a burst of writes.
9. **`issue unarchive`**, so archive can be undone.
10. **`comment update` / `comment delete`** for one's own comments.
11. **`notification list`** (the Linear inbox), for an inbox screen.
12. **`signedUrlsExpireAt`** in results made with `--signed-urls`, instead of the
    TUI inferring expiry from when it spawned the call.
13. **Several teams in one list**: `issue list --team A --team B`.

Doc nit: linear-cli's `docs/design.md` still calls linear-tui an OpenTUI app; it is
Rust + ratatui.
