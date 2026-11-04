# HerdR Terminal

`herdr-terminal` (product **HerdR Terminal**, binary `ht`) is the TUI client of
Herdr Core. It is a sibling of Herdr App. It owns the terminal screen and
workspace organization; Core owns the programs and the facts about them.

If `AGENTS.local.md` exists, read it too: the maintainer's local rules.

**Status: implemented, unreleased.** The client implements the core connection,
local VT and terminal lifecycle, the spaces/tabs/panes workspace, its
`herdr-terminal` document with restore and multi-screen sync, agent awareness
(status, sidebar, local-VT peek, seen/pin, alerts), the Core-wide terminal
manager (`prefix t`; Show/Hide and a read-only Preview), the machine manager
(`prefix m`; add/connect/rename/remove Core machines and start `machine/t7`
terminals), the file viewer/editor (`prefix o`; paged reads, `$VISUAL`/`$EDITOR`,
line diff and content-checked write-back), and the daily-driver input surface:
mouse routing and split drags, bounded local scrollback with search, selection
with OSC 52/native copy and bracketed paste, the command palette, and startup
settings with default/dark/light themes. The local VT enables grapheme
clustering as Core does, and new terminals start at the grid they will attach
at. Settings are read at startup only: there is no settings UI or live reload.
The tests exist and pass. A file picker, install scripts and accessibility
remain planned.
The design intent is in the
[core reference](docs/01-core-and-app-reference.md) and the
[architecture](docs/02-architecture.md). Read both before proposing code; do
not invent a build or scaffolding that contradicts them.

Read the code. This file holds the rules, the traps, and the decisions that are
easy to get wrong.

## The rule

Core is a runtime, not a multiplexer. Spaces, tabs, panes, layout, focus,
themes and keybindings belong here, not in Core. Core owns PTYs, terminal
records, raw output, attach, events, status, documents and machines.

## Rules that aren't in the code

- **Status is only OSC 7501.** No screen scraping, no agent registry, no
  vendor-specific logic. This is the rule most likely to be broken "helpfully".
- **One long-lived framed connection.** Do not spawn `hc` per user action.
  Reconnect; the programs keep running.
- **Feature-gate on `hello`.** Check generation, the minimum version, methods
  and features; protocol generation 1 only grows. The minimum floor is the only
  version check; optional capabilities use features, never version strings.
- **Supported Core range.** Core 0.1.17 or newer at protocol generation 1,
  with the required methods and features (including `resize_notifications`).
  Older or unreadable versions are refused at connect with an update hint.
  Other optional capabilities are gated on `hello.features`, never a version
  string.
- **Own state.** The workspace document is `herdr-terminal`; labels are
  `herdr.terminal.*`. Do not read or write Herdr App's `herdr-app` document,
  `herdr.space` or `herdr.tab`. Share only the cross-client `herdr.seen` and
  `herdr.pinned` conventions.
- **Local daily-use state.** Scrollback, search, selection and theme stay in
  memory. Never write them to the shared `herdr-terminal` document or the
  per-screen view; those hold layout and view state only.
- **Settings are startup-only.** Read `config.json` once at launch. Do not add
  live reload or a settings UI without a separate design.
- **Local VT.** Core sends raw bytes and a first paint, not draw commands.
  Run libghostty-vt locally; render cells yourself; encode keys locally.
- **Match Core on cell width.** Enable DEC mode 2027 (grapheme clustering) as
  a reset default in the local VT, as Core and Herdr App do. Variation-selector
  emoji must form one cluster two cells wide, followed by a zero-width spacer
  cell; a disagreement in cell width shifts cursor placement, selection and
  search. RIS keeps the default because it is a reset default.
- **Send the attach size at spawn.** Every `terminal.run` path sends the
  optional `size` when it knows the grid the terminal will attach at, so a
  program that reads its size once starts with the right layout. Use the exact
  grid the terminal will attach at, not a guess, and omit the field when it is
  unknown. The supported Core floor applies it, so no version gate is needed.
- **Start directory.** A new local terminal starts in the focused pane's
  Core-reported cwd when Core has one, otherwise the client process's cwd, and
  `$HOME` when that is `/` or unreadable. The first shell has no pane, so it
  uses the client rule; an explicit `ht PROG` in a restored workspace follows
  the focused pane too. A start with `on` sends no cwd and uses the machine's
  home; never send a local path to a remote start. A replacement shell uses the
  exited terminal's cwd, then the client rule for a local pane.
- **Observer attaches are read-only.** The connection requires
  `resize_notifications`, so an observer attach always carries `observe` and
  `resize_notifications`; the attach reply must still confirm it.
  An observer applies each RESIZE before the DATA that follows it and pays no
  credit for it; it sends no keys, mouse reports, RESIZE, FOCUS or FACTS. If
  confirmation is absent, fail clearly; never fall back to a normal attach,
  which would take over the grid. Normal attaches are unchanged. The terminal
  manager's Preview uses observer attach; the local-VT agent peek is a
  different feature.
- **Terminal manager inventory is the event model.** The Core-wide manager
  (`prefix t`) lists every live terminal from the `events.subscribe` model,
  including terminals that are not in this workspace. Never call
  `terminal.read`, never build the inventory from screen text, and never infer
  status. Show adopts the existing address (`owned=false`, no `terminal.run`,
  no spawn or respawn); a shown address focuses instead of adding a second
  pane. Hide detaches the stream and removes the pane, never `terminal.kill` or
  `terminal.rm`. Preview is an observer attach only: read-only, no keys,
  mouse, RESIZE, FOCUS or FACTS, and no workspace or document write. Selection,
  filter and scroll are `UiState`, not the document or the per-screen view.
- **Hide clears only this screen's labels.** Hide always removes the pane from
  the shared layout, but it clears the four identity labels only when the
  terminal carries this client's `herdr.client` value and its pane label is in
  this screen's id namespace. Another `ht` screen's labels stay so that screen
  can recover the terminal; a later label-based recovery may re-add it, so
  Hide is durable across restarts only for terminals this screen created.
  Durable cross-screen hide needs a shared-document marker, which this feature
  does not have. A manager-shown pane restores by address as unowned: `x`/`e`
  detach, `X` kills.
- **Core owns the machine line.** The machine manager (`prefix m`; palette
  "machine manager", action `machines`) only calls `machine.list`, `add`,
  `connect`, `rename` and `rm` on the one long-lived connection. Never dial,
  implement SSH or a `via` path, retry a line, or add a private remote
  reconnect loop: Core owns dialing, backoff, mirror replacement and relayed
  streams. Never spawn `hc` per action. A core without `machine.list` is
  reported inside the overlay; never call the missing method.
- **Machine manager refreshes only while open.** It calls `machine.list` when
  the overlay opens, after each action, and on a bounded timer while it stays
  open; a closed manager never polls. Core has no machine-state event.
  Selection, filter and scroll are `UiState`, not the document or the
  per-screen view. Opening the terminal manager closes it, and every key is
  consumed by the overlay, prompt or confirmation.
- **Machine starts and remote addresses.** `machine.add` passes the typed
  target unchanged, including `user@host:port`; never parse, rewrite or add a
  `via` path. Only a start from the machine manager sends `terminal.run` `on`;
  it keeps this client's normal labels. The returned `machine/t7` address is
  stored unchanged, attaches through the local Core, and is an ordinary pane:
  input, resize, focus, FACTS, terminal-manager Show/Hide and Preview all
  apply. Persist the full qualified address in the local document and show
  the machine in pane/status names; never assume a short `t7` is unique
  across machines.
- **Renames follow the pane label.** Core renames a machine as
  `removed`+`created` or in a `missed` snapshot. Re-address the existing pane
  by its `herdr.terminal.pane` label, keep its stream and final screen, and
  write the new qualified address; never spawn a replacement. On `missed`,
  refetch and merge the document before reconciling the snapshot. If the new
  address is already shown, do not create a second pane: refuse the move, let
  the old pane leave only after Core no longer has its old address, and keep
  the pane that holds the address.
- **Local documents only.** Workspace state, including remote panes, stays in
  the local `herdr-terminal` document. This feature reads and writes no remote
  document (any later remote document use must call `doc.get`/`doc.set` with
  `on`), and never touches the App document or `herdr.space`/`herdr.tab`.
- **Core owns file IO.** `file.read`, `file.write` and `file.edit` go over the
  one long-lived connection, for local and remote paths alike. Never open a
  remote path directly, never fall back to local path IO, and never spawn
  `hc` per action. The three methods are required at connect; the minimum
  Core stays 0.1.17 and a missing method is refused by the `hello` check,
  with no fallback.
- **File contents stay in the session.** The viewer window, the first full read,
  the editor buffer and the diff live only in the runtime file session; the
  editor's temp file is the one local copy. Never write file contents to the
  `herdr-terminal` document, a label, another client's document or a Core
  document.
- **The viewer pages.** `prefix o` reads through `file.read` with `offset` and
  `limit`; one page is 400 lines. Never call an unbounded read for the viewer
  or load a large file whole. An unbounded read is only for `e` and the
  write-back check; it must pass the completeness predicate. A truncated
  reply (`last < lines`) may be paged, but it is never edited, compared or
  written.
- **Editor temp and TUI restoration.** `e` reads the whole file through Core,
  writes a unique local temp (`ht-<pid>-<serial>-<basename>`, create-new,
  0600 where supported, basename and extension preserved), and runs `$VISUAL`
  then `$EDITOR` with the temp path appended. Split the value on whitespace;
  do not shell-quote it and do not add a `vi` fallback. Suspend the TUI and
  restore the alternate screen, raw mode, keyboard enhancement, paste, focus
  and mouse, including when the editor fails or exits nonzero. Remove the
  temp on every path.
- **Write-back is content-checked, not atomic.** Core has no file version or
  etag. Re-read the whole file before writing and compare it with the first
  read; a detected difference requires the explicit `o` overwrite. The
  comparison and the write are separate requests, so a change made between
  them is not detected and can be overwritten. The shared workspace document
  is different: `doc.get` returns a version and `doc.set` takes the expected
  version, so its updates are optimistic compare-and-set, not
  read-compare-write.
  `w`/Enter and `o` both refuse a truncated re-read before comparing or
  writing. A failed write keeps the diff and buffer. Use `file.edit` only for
  one exact contiguous replacement; otherwise use `file.write` with base64
  data.
- **No directory listing.** The path prompt is the picker. Refuse a trailing
  `/` or `\` without a read; other directory errors come from Core. A file
  picker/list remains planned. There is no embedded editor, merge or
  three-way resolution, and no direct path IO for the file itself.
- **Reconnect semantics.** A dropped connection is not a dead program. On
  `missed`, resnapshot; on `lost`/`restarted`, reattach. When a program in a
  terminal this screen started exits, close its pane: stop the stream, remove
  the finished record with `terminal.rm` (best effort), and remove the pane; a
  tab's last pane gets one fresh shell in the same space and tab (same machine
  for `machine/t7`), a background tab's replacement must not switch the active
  space or tab, and a replacement that exits almost immediately is not replaced
  again. A restored or adopted record that was already finished keeps its final
  screen until `x`. Never assume the program died from a dropped connection.
- **No mods in v1.** If mods come later, use the old TUI's manifest +
  external-command model (`herdr-plugin.toml`, actions/events/panes/link
  handlers, CLI/socket), not Herdr App's JS/GPUI runtime.
- **Pure decisions, IO at the edges.** Keep the model, layout, commands and
  state transitions testable without PTYs.
- **TDD.** One good test per documented behavior; end-to-end with real PTYs
  where it matters. The suite stays in seconds; no flaky sleeps.
- **Isolated tests.** Use a unique `HC_SESSION` and scratch XDG paths under
  `/var/tmp`. Never point tests at a real core, app or running agent.
- **Lean.** Small files, few dependencies, no abstraction before a second
  real caller. Keep source files under ~32 KB where practical.
- **Do not modify `herdr-core` or `herdr-app` from this repo.** Core changes
  are separate work under Core's rules; feature-gate instead of forking.
- **Platforms.** macOS, Linux, Windows. Gate per OS only where behavior really
  differs; test the quirk on its platform. Do not fake input on a real display.

## Commands

The Cargo workspace builds and tests. The suites cover the protocol, VT,
single-pane lifecycle, workspace, persistence, agents, the daily-driver input
layer (mouse, scrollback/search, clipboard, palette, config and themes),
grapheme width, initial terminal size, the terminal manager, the machine
manager and remote recovery, the file viewer/editor and write-back, and
observer attach/resize ordering.

```sh
cargo build --locked
cargo test --locked
cargo check --locked
cargo test --locked <filter>
```

`just` wraps the same commands:

```sh
just build
just test
just check
just fmt
just notices
just package
just release-prepare <version>
just release-publish <version>
```

`just check` runs `cargo fmt --check`, clippy with `-D warnings` and the test
suite. `just notices` regenerates `THIRD-PARTY-NOTICES` after a dependency
change; `just package` builds the macOS release archive in `dist/`. The release
helpers are maintainer-only: `release-prepare` bumps the package version and
re-runs the checks without committing; `release-publish` pushes the branch,
waits for its CI run, then tags and pushes the tag. Normal work does not run
them.

Toolchain: Rust 1.95.0 (MSRV), `just`; Zig 0.16.0 only when building the
libghostty-vt binding. Stack: ratatui 0.30, crossterm 0.29,
libghostty-vt, unicode-width/segmentation.

Keep this a separate Cargo workspace; do not add it to `herdr-core`'s
workspace.

## Layout

See [docs/02-architecture.md §4.2](docs/02-architecture.md#42-modules). Current:

```text
src/
  main.rs, lib.rs       # CLI; terminal setup and restore
  core/                 # discovery, framed client, events, reconnect
  screen/               # single-pane screen: lifecycle, play, pointer, view
  vt/                   # VT binding: screen, graphemes, keys, mouse, scroll, search, selection
  model.rs              # pure event/terminal model
  workspace/
    model.rs            # pure workspace state
    model/              # document schema, merge, restore, ids, local view
    geometry.rs         # split tree, geometry, hit testing
    commands.rs, keys.rs # command table and bindings
    palette.rs          # pure palette entries and filtering
    agents.rs           # pure agent facts, ordering, alerts
    manager.rs          # pure Core-wide inventory, sort/filter, labels
    machines.rs         # pure machine rows, sort/filter, remote addresses
    file.rs             # pure file context, paging, diff, editor and write rules
    file/tests.rs       # file model tests
    runtime/            # event loop, attach, input, mouse, overlays, reconnect, agents, manager, machines
    runtime/file.rs     # file reads, editor, diff and write-back
    ui.rs               # ratatui composition
    ui/machine_ui.rs    # machine manager panel, prompt and confirmation
    startup.rs          # document claim and restore
    local.rs, state.rs  # local view and UI state
    sync.rs             # document CAS state
  tui.rs, term.rs, session.rs, probe.rs
  config.rs, theme.rs, clipboard.rs  # settings, themes, copy paths
crates/                 # the VT binding crate
tests/                  # probe, screen, workspace, fake-core
```

## Reading order

1. [Core and App Reference](docs/01-core-and-app-reference.md).
2. [Architecture](docs/02-architecture.md).
3. **Herdr Core** (repository: `herdr-core`): the schema README for protocol, attach, events and documents.
4. **Herdr App** (repository: `herdr-app`):
   - `hc.rs`, `core_bin.rs` — connection and discovery.
   - `model.rs`, `layout.rs` — workspace/layout patterns.
   - `mirror.rs`, `mirror_sync.rs` — document CAS patterns.
   - `terminal.rs`, `vt.rs` — attach, VT and input patterns.
   - the labels document — cross-client label rules.

## Traps

- The product is **HerdR Terminal**; code identifiers are `herdr-terminal` and
  `ht`.
- Do not write Herdr App's labels; that makes the App adopt TUI terminals into
  its spaces.
- Do not use screen text for status, turn, title or foreground; Core is the
  source.
- Do not spawn a Core or `hc` per action; keep one connection.
- Do not run a bare Core/App against a real home; tests are isolated.
- Do not add mods, an embedded editor, Markdown or images in v1; see the
  architecture's non-goals.
- The checked-in `justfile` wraps the `cargo` commands above; CI runs them
  directly rather than through `just`.

## Contributing

Work on a branch and open a pull request to `master`. CI must pass; the
maintainer approves. Commit messages: lowercase conventional commits, single
line, no AI co-author lines. Only the maintainer releases. `CHANGELOG.md` is
written at release time, one user-visible line per change; normal work does not
edit it.

## Next step

Pick up the remaining planned features in `docs/02-architecture.md` §5:
the file picker and directory listing, install scripts and CLI docs,
accessibility, mods, and a settings UI or live reload if one is designed. Keep
the same rule: prove protocol, VT, input and recovery behavior with tests
before UI chrome.
