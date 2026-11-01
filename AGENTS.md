# HerdR Terminal

`herdr-terminal` (product **HerdR Terminal**, binary `ht`) is the TUI client of
Herdr Core. It is a sibling of Herdr App. It owns the terminal screen and
workspace organization; Core owns the programs and the facts about them.

If `AGENTS.local.md` exists, read it too: the maintainer's local rules.

**Status: design phase.** The Cargo workspace and `ht` binary skeleton exist;
the TUI and tests are not implemented yet. The design is in
[docs/01-core-and-app-reference.md](docs/01-core-and-app-reference.md) and
[docs/02-architecture.md](docs/02-architecture.md). Read both before proposing
code. Do not invent a build or scaffolding that contradicts them.

Read the code once it exists. This file holds the rules, the traps, and the
decisions that are easy to get wrong.

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
- **Local VT.** Core sends raw bytes and a first paint, not draw commands.
  Run libghostty-vt locally; render cells yourself; encode keys locally.
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

The Cargo workspace exists. `cargo build`, `cargo test` and `cargo check`
work; there are no tests yet.

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

See [docs/02-architecture.md §4.2](docs/02-architecture.md#42-modules). Planned:

```text
src/
  main.rs
  core/       # framed client, attach, events, reconnect
  model/      # pure workspace state
  layout/     # split tree, geometry
  vt/         # libghostty-vt binding
  ui/         # ratatui composition
  input/      # crossterm, key/mouse encoding
  state/      # documents, local view
  commands/   # command table, palette, CLI
  theme/
  config/
  cli/
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
- Do not assume a build exists; this is the design phase.

## Contributing

Work on a branch and open a pull request to `master`. CI must pass; the
maintainer approves. Commit messages: lowercase conventional commits, single
line, no AI co-author lines. Only the maintainer releases. `CHANGELOG.md` is
written at release time, one user-visible line per change; normal work does not
edit it.

## Next step

Start Phase 0 (connection + one terminal) per `docs/02-architecture.md`. Do
not start with UI chrome; prove attach, render, input and reconnect first.
