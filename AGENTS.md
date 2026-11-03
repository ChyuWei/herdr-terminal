# HerdR Terminal

`herdr-terminal` (product **HerdR Terminal**, binary `ht`) is the TUI client of
Herdr Core. It is a sibling of Herdr App. It owns the terminal screen and
workspace organization; Core owns the programs and the facts about them.

If `AGENTS.local.md` exists, read it too: the maintainer's local rules.

**Status: implemented, unreleased.** The client implements the core connection,
local VT and terminal lifecycle, the spaces/tabs/panes workspace, its
`herdr-terminal` document with restore and multi-screen sync, agent awareness
(status, sidebar, peek, seen/pin, alerts), and the daily-driver input surface:
mouse routing and split drags, bounded local scrollback with search, selection
with OSC 52/native copy and bracketed paste, the command palette, and startup
settings with default/dark/light themes. The local VT enables grapheme clustering
as Core does, and new terminals start at the grid they will attach at. Settings
are read at startup only: there is no settings UI or live reload. The tests
exist and pass. Machines, files, packaging and accessibility remain planned.
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
- **Feature-gate on `hello`.** Check generation, version, methods and features;
  protocol generation 1 only grows. Do not assume a version.
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
  unknown. Older cores ignore it; do not add a version gate.
- **Observer attaches are read-only.** `terminal.attach` takes `observe` and
  `resize_notifications`; request them only when `hello.features` advertises
  `resize_notifications` (no version gate), and require the reply to confirm.
  An observer applies each RESIZE before the DATA that follows it and pays no
  credit for it; it sends no keys, mouse reports, RESIZE, FOCUS or FACTS. If
  the feature or confirmation is absent, fail clearly; never fall back to a
  normal attach, which would take over the grid. Normal attaches are unchanged.
  This is attach-layer plumbing for a future peek; no peek UI uses it yet.
- **Reconnect semantics.** A dropped connection is not a dead program. On
  `missed`, resnapshot; on `lost`/`restarted`, reattach; on `finished`, drop
  the stream. Never assume the program died.
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
grapheme width, initial terminal size, and observer attach/resize ordering.

```sh
cargo build --locked
cargo test --locked
cargo check --locked
cargo test --locked <filter>
```

Planned `just` recipes (not created yet):

```sh
just build
just test
just check
just fmt
```

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
    runtime/            # event loop, attach, input, mouse, overlays, reconnect, agents
    ui.rs               # ratatui composition
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
- Do not assume a `just` file exists; use the `cargo` commands above.

## Contributing

Work on a branch and open a pull request to `master`. CI must pass; the
maintainer approves. Commit messages: lowercase conventional commits, single
line, no AI co-author lines. Only the maintainer releases. `CHANGELOG.md` is
written at release time, one user-visible line per change; normal work does not
edit it.

## Next step

Pick up the remaining planned features in `docs/02-architecture.md` §5:
machines, files, distribution, accessibility, mods, and a settings UI or live
reload if one is designed. Keep the same rule: prove protocol, VT, input and
recovery behavior with tests before UI chrome.
