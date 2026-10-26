# linear-tui — design (draft, 2026-10-26)

A terminal UI for Linear in Rust and ratatui. It gets every byte of Linear data
from `linearctl … --json` and writes only through `linearctl`; it never calls
api.linear.app and never sees the API key. Anything a person does here, an agent
can do with the CLI — and the TUI shows the exact command for each action (see
[Command parity](#command-parity)).

Built against linearctl **0.1.0**. Every `linearctl` invocation in this file is
checked against `linearctl describe` by `cargo test` (`src/contract.rs`);
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
 │   ↑ AppEvent            ↓ Effect                    │
 │ App state (Elm-style update; pure, unit-tested)     │
 │   ↓ Effect                                          │
 │ tokio runtime                                       │
 │   ctl::Runner ── spawn ──▶ linearctl … --json  ─────┼──▶ api.linear.app
 │   images::Fetcher ── https GET (signed URL) ────────┼──▶ uploads.linear.app
 │   cache (memory + $XDG_CACHE_HOME/linear-tui)       │
 └─────────────────────────────────────────────────────┘
```

Crates: `ratatui` + `crossterm`, `ratatui-image` + `image`, `tokio`, `serde` /
`serde_json`, `pulldown-cmark`, `unicode-width` + `textwrap`, `tui-textarea`,
`uuid` (v4), `reqwest` (rustls, HTTPS only), `directories`, `toml`, `tracing`
(to a log file, never the terminal).

Modules:

| module | does | tested by |
|---|---|---|
| `ctl::args` | builds argv from typed requests (`ListQuery`, `IssueEdit`, …); pure | unit + contract test against the describe snapshot |
| `ctl::runner` | spawns, enforces timeouts and concurrency, maps exit codes | fake `linearctl` binary driven by a scenario file |
| `model` | serde types for every result; unknown fields ignored, missing ones defaulted | fixtures captured from linearctl 0.1.0 |
| `markdown` | Markdown → layout blocks (text, image, code, table, rule) | golden tests incl. CJK width, hostile input |
| `images` | fetch, cache, decode, size | local HTTPS stub; halfblocks protocol in tests |
| `store` | query cache, lookups, freshness | unit |
| `app` | state + `update(event) -> effects` | unit, no terminal |
| `ui` | one `render` per screen | ratatui `TestBackend` snapshots (`insta`) |

### Talking to linearctl

- Binary: `$LINEARCTL`, else `linearctl` in `config.toml`, else `PATH`.
- On start, run `linearctl describe --json` (local, ~5 ms, no request). Check that
  every command and flag the TUI builds exists — the same check `cargo test` runs on
  this doc — and refuse to start with one clear message if not. The same result
  turns on optional features when linearctl grows them (`watch`, `serve`; see
  [gaps](#what-linearctl-lacks-proposed)).
- Each call: `tokio::process::Command` with `kill_on_drop`, stdin closed (or the
  body for `--description-file -`), stdout and stderr captured, `--timeout 15`
  plus an outer 20 s deadline. The environment is inherited unchanged; the TUI
  never sets or reads `LINEAR_API_KEY`.
- Into a pipe the JSON is one compact line. Parse stdout whatever the exit code:
  failures also print `{"error", "usage"}` there (*verified*). The stderr hint
  ("3 shown, more remain; --cursor …") is ignored.
- At most 4 calls in flight; reads are cancelled (process killed) when the screen
  that asked for them goes away. Every request carries a generation number, and a
  result for a stale filter or a closed issue is dropped.

| exit | TUI reaction |
|---|---|
| 0 ok | apply |
| 1 failed | toast with `error`; "not found: …" on a detail closes it and drops the row |
| 2 usage | a TUI bug: toast "internal error", log the full argv |
| 3 partial | not produced by the commands we use (`api query` only); treated as 1 |
| 4 busy | retry after 200 ms, 400 ms, 800 ms, then 1 |
| 5 auth | full-screen notice: run `linearctl auth login --yes` with the key on stdin in a shell, then press `r`. The TUI does not take the key itself |
| 6 rate-limited | pause all reads until `retryAt`, countdown in the status bar; a pending write stays in its dialog for the user to retry |

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

Long text goes through stdin (`--description-file -`, `--body-file -`), never argv,
so it stays out of `ps` and has no length limit.

Single-field quick edits (state, priority, assignee) skip the confirm dialog — the
dry run still runs and its error, if any, is shown — and can be undone with `u`
(the TUI remembers the previous value and issues the reverse update).

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
| `f` | filter panel |
| `o` | sort cycle: updated → created → priority |
| `1`…`9` | saved views |
| `s` `p` `a` `L` | quick edit state / priority / assignee / labels of the selected issue |
| `A` | assign to me |
| `c` | create issue |
| `x` | archive (confirm) |
| `O` | open in browser |
| `n` at the bottom | load the next page (`--cursor`); also automatic when scrolling past the end |

**Views** are named filter sets in `~/.config/linear-tui/config.toml`, each the
`issue list` flags it stands for:

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
| `e` | edit form; `E` description in `$EDITOR` |
| `s` `p` `a` `L` `A` `x` | as in the list |
| `c` | comment (`tui-textarea`; `Ctrl-e` moves the draft to `$EDITOR`) |
| `Tab` | focus the next link, sub-issue, relation or image; `Enter` follows it (issue → detail, URL → browser, image → full screen) |
| `i` | toggle images (off → alt text only) |
| `v` | the focused image full screen |
| `d` | download the focused upload |
| `C` | load all comments (when more than shown) |
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
   labels, assignee). Free.
2. `Enter` in the box runs a server-side full-text search (titles, descriptions,
   comments): `linearctl issue list --query <TEXT> --limit 50 --json`, with the
   current view's filters added when "within view" is toggled (`Ctrl-v`). Results
   open as a temporary *Search* view; `Esc` returns to the previous one.

A client-side token bucket allows 20 searches a minute, leaving 10 for agents on
the same key; above it the box says when the next search is allowed.

### 4. Create

`c` opens a form: team (default from config), title, priority, state, assignee,
labels, project, parent, and the description (`Ctrl-e` for `$EDITOR`). Pickers
use the cached lookups. Submit runs

```
linearctl issue create --team ENG --title "…" --priority high --label bug --description-file - --id <uuid> --dry-run --json
linearctl issue create --team ENG --title "…" --priority high --label bug --description-file - --id <uuid> --yes --json
```

and opens the new issue's detail.

### 5. Edit

- Quick edits: `s` state picker (states of the issue's team, in position order),
  `p` priority (0–4), `a` assignee (`me`, `none`, or an email), `L` labels
  (multi-select; sends `--add-label` / `--remove-label` for the difference, so a
  label added concurrently by someone else survives).
- `e` full form; only changed fields become flags.
- `E` description: the current text in a temp file (0600) in `$EDITOR`; on save,
  `linearctl issue update <ID> --description-file - --dry-run --json`, then `--yes`.

Concurrent edits: before writing a description, the TUI re-reads `updatedAt` with
`linearctl issue show <ID> --fields updatedAt --comments=0 --json` and, if it moved
since the editor opened, shows both versions and asks. One extra request; a proper
fix is a precondition flag in linearctl (gap 5).

### 6. Comment

`c` in the detail →
`linearctl comment create <ID> --body-file - --id <uuid> --dry-run --json`, then
`--yes`. The new comment is appended from the result. Replying inside a thread
needs a `--parent` flag linearctl does not have (gap 6); until then a reply is a
top-level comment that quotes the parent.

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

- The visible list refreshes every **120 s** with `--updated-since`, only while the
  terminal has focus (crossterm focus events) and the list is on screen. After 10
  minutes with no key press the timer stops until the next key.
- `--updated-since` cannot see an issue that *left* the view (moved to Done,
  reassigned): its row stays until the next full reload. A full reload of the first
  page runs every 10 minutes while active, and on `r`. `watch` would fix this (gap 1).
- When linearctl has `watch` (detected from `describe`), the TUI runs one watcher
  for the visible view and drops both timers; they stay as the fallback when the
  watcher exits. linear-cli reports (2026-10-26) that Linear's GraphQL
  subscriptions accept a personal API key, events arriving 0.07–4.6 s after a
  change, so `watch` will push rather than poll where it can.
- Details are cached for 2 min; nothing is prefetched.
- Lookups: once per day per team.

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

- In memory: results keyed by the argv that produced them, with fetch time.
- On disk, for instant start: the last first page of each view and the lookups, in
  `$XDG_CACHE_HOME/linear-tui/<org urlKey>/`, 0600. At start the cached page shows
  at once, marked stale, and is revalidated. Issue details are not persisted.
- `--fields` keeps list payloads small; details always fetch all fields.

## Testing

Tests ship with the first line of code; CI fails below 85% line coverage
(`cargo llvm-cov`), with `ctl`, `model`, `markdown` and `images` held to 90%.

- **Contract**: the describe snapshot (`tests/fixtures/linearctl-describe-0.1.0.json`)
  is checked against every argv the builders can produce, and against this doc
  (`cargo test`). `LINEARCTL_LIVE=linearctl cargo test` checks the installed one.
- **Model**: fixtures captured from linearctl 0.1.0 (list page, detail with images
  and comment threads, every error shape, dry-run and write results), with personal
  data replaced.
- **Runner**: a fake `linearctl` (a test binary reading a scenario: stdout, stderr,
  exit code, delay) for timeouts, cancellation, each exit code, busy retries.
- **App**: `update` is pure; key sequences in, state and effects out.
- **UI**: `TestBackend` snapshots per screen, at 80 and 160 columns.
- **Markdown**: golden files, including CJK, nested lists, tables wider than the
  pane, and hostile input (escape sequences, bidi overrides, 1 MB descriptions).
- **Images**: a local HTTPS stub serving fixtures with 200/401/oversized/not-an-image
  answers; protocol fixed to halfblocks so output is deterministic.
- **Live smoke** (opt-in, `LINEAR_TUI_LIVE=1`, reads only): list, show with signed
  URLs, fetch one image.

## Milestones

1. Skeleton: runner, describe check, issue list with views, detail without images.
2. Markdown renderer and comments.
3. Images.
4. Filter panel, search, lookups cache.
5. Writes: quick edits, create, edit, comment, archive.
6. Live updates and `serve` once linearctl has them.

## What linearctl lacks (proposed)

Sent to the linear-cli owner as a list; the TUI works around each until it lands.
Ordered by how much the TUI needs it.

1. **`linearctl watch --json` (NDJSON)** taking `issue list`'s filters, emitting
   `{"type":"upsert","issue":{…}}` and `{"type":"remove","id":…}` when an issue
   stops matching. `--updated-since` already gives upserts; the TUI cannot see
   removals without a full reload. One watcher per key also beats each consumer
   polling. *Accepted*: linear-cli will build it on Linear's (undocumented)
   GraphQL subscriptions with a polling fallback, re-checking the filter for each
   changed issue to emit removals; the NDJSON shape comes before the build.
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
