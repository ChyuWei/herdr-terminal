# HerdR Terminal — Architecture

**Status:** draft, 2026-11-02
**Product:** HerdR Terminal
**Code:** `herdr-terminal` (repo, package, core document); binary `ht`
**Tagline:** The terminal client for Herdr Core.
**Reference:** [01-core-and-app-reference.md](01-core-and-app-reference.md)

## 1. Summary

HerdR Terminal is a terminal UI client of **Herdr Core**, the runtime that owns PTYs, terminal lifetime, raw output relay, events, status, documents and machines. It is a sibling of **Herdr App** (the GPUI desktop client) and a replacement for **Herdr 1.x** (the old monolithic TUI in the `herdr` repository; reference only).

The split is deliberate:

- **Core owns the programs and the facts about them.** Terminals, raw bytes, attach streams, events, status, labels, documents, machines, exec/files.
- **HerdR Terminal owns presentation and workspace organization.** Spaces, tabs, panes, layout, focus, themes, keybindings, and the terminal screen itself.
- Core has no concept of spaces, tabs, panes or layouts. The TUI never infers terminal status from screen text.

v1 has **no mods**. If mods are added later, they follow Herdr 1.x's **manifest + external-command** model, not Herdr App's JS/GPUI runtime. See [§6.3](#63-postponed) and [§6.4](#64-not-supported).

## 2. Goals and non-goals

### Goals

- A terminal-native client that covers the daily Herdr workflow: run terminals, organize spaces/tabs/panes, watch agents, switch machines, inspect files.
- One long-lived connection to core; reconnect without losing the programs.
- A pure, testable workspace/layout model; IO kept at the edges.
- Correct terminal rendering and input through a local VT, with credit-based flow control.
- Its own persisted workspace, so multiple HerdR Terminal screens can share a layout without touching Herdr App's state.
- A codebase that can grow without inheriting Herdr 1.x's monolith or Herdr App's GPUI/JS stack.

### Non-goals

- Reimplementing Core (PTYs, servers, protocols, detection).
- Reusing Herdr 1.x code.
- Porting Herdr App's GPUI/JS mod runtime.
- A code editor, Markdown renderer, image viewer, or pixel UI.
- Sharing Herdr App's `herdr-app` document or `herdr.space`/`herdr.tab` labels.
- Screen-based agent detection of any kind.

## 3. Relationship to Core and App

```
Herdr App (GPUI) ──┐
                   ├── framed JSON-RPC ──> Herdr Core ── PTY ──> programs / agents
HerdR Terminal ────┘                         │
    │                                        ├── events / status / labels
    ├── local libghostty-vt                  ├── documents (opaque, versioned)
    ├── ratatui UI                           └── machines (remote cores)
    └── document `herdr-terminal`
```

| Program | Owns | Does not own |
|---|---|---|
| Herdr Core | PTYs, terminal records, raw output, attach, events, status, documents, machines, exec/files | Spaces, tabs, panes, layout, rendering, key encoding |
| Herdr App | GPUI UI, its `herdr-app` doc, its labels, JS/GPUI mods | Core runtime, terminal lifetime |
| HerdR Terminal | TUI UI, its `herdr-terminal` doc, its labels, local VT/input | Core runtime, Herdr App's workspace |

Rules:

- **Core is a runtime, not a multiplexer.** Do not move layout into core.
- **Core never sends draw commands.** The TUI runs its own VT and composes its own cells.
- **Status is only OSC 7501.** No screen scraping, no agent registry, no vendor logic.
- **A client owns its document and label namespace.** Do not read another client's private state as yours.
- **Feature-gate on `hello`.** Check generation, version, methods and features; generation 1 only grows.

## 4. Architecture

### 4.1 Process model

One TUI process:

- a **core reader** thread (or async task) that parses framed messages and pushes calls, events and attach bytes into channels;
- a **UI/event loop** that reads crossterm events, updates the state, draws with ratatui, and sends input/control frames;
- a **persistence** path for the workspace document and local view;
- a **timer** for redraw coalescing, heartbeats and reconnect backoff.

Start with threads and channels; introduce an async runtime only if a measured need appears. The core connection is long-lived; never spawn `hc` per user action.

### 4.2 Modules

| Module | Responsibility |
|---|---|
| `main` | Terminal setup/restore, event loop, shutdown |
| `core` | Core discovery, framed connection, calls, attach streams, events, reconnect |
| `model` | Pure workspace state: spaces, tabs, panes, layout, focus, zoom, agents; no IO |
| `layout` | Split tree, geometry, pane frames, hit testing, resize |
| `vt` | libghostty-vt binding: terminal state, render state, cells, key/mouse encoding |
| `ui` | ratatui composition: tab bar, pane borders, status bar, sidebar, palette, dialogs, toast, peek |
| `input` | crossterm events, keybindings, mouse, paste, focus, resize |
| `state` | App state, per-screen view, document schema and sync |
| `commands` | Single command table used by keys, palette, menus and CLI |
| `theme` | Colors, styles, default palette, 16/256/truecolor mapping |
| `config` | Local settings, keybindings, core path |
| `cli` | `ht` subcommands: status, commands, `--skill`, version |

The model/layout/commands modules must be pure and unit-testable without a PTY. IO lives in `core`, `vt`, `ui` and `main`.

### 4.3 Rendering pipeline

1. The attach stream delivers the **first paint** and then raw program bytes.
2. Feed bytes into the local VT; it owns screen, scrollback, cursor, modes, title and responses.
3. On redraw, update the VT render state and iterate visible rows/cells.
4. Map each cell's symbol, style, foreground/background and attributes into a ratatui `Buffer` cell; honor wide cells and continuation cells.
5. Compose the TUI around the terminal area: tab bar, pane borders, status bar, sidebar, overlays.
6. Diff and present; use synchronized output when the terminal supports it; place the cursor only for the focused terminal.
7. Send **FACTS** when colors or cell size change, **RESIZE** when a pane changes size, **FOCUS** when focus changes.

The TUI owns the screen. It does not ask core for frames or scrape `terminal.read`.

### 4.4 Input

- crossterm events: key, mouse, paste, focus, resize.
- Enable keyboard enhancement flags and bracketed paste; restore on exit.
- Encode keys through the local VT so kitty keyboard protocol and Windows win32-input-mode are honored.
- Send keys as **DATA** on the terminal's attach channel.
- Mouse: wheel and click first; drag/resize later. Mouse reporting is terminal-dependent.
- Clipboard: OSC 52 and/or native clipboard, with a configurable fallback (open question).

### 4.5 State and persistence

**Shared workspace** is stored in a document named `herdr-terminal`:

- one document per core, with machine-qualified ids for remote spaces;
- `version` field for schema migration;
- spaces: id, name, folder, machine;
- tabs: id, name, layout tree;
- layout leaves reference terminal addresses and labels; no local focus/zoom/selection;
- writes use `doc.get`/`doc.set` with optimistic concurrency; a `conflict` means re-read and merge.

**Local view** stays on the machine running the TUI: focus, zoom, selection, scroll position, sidebar state, theme override. It is not shared across screens.

**Labels** identify the TUI's own terminals after a restart:

| Label | Meaning |
|---|---|
| `herdr.client=herdr-terminal` | Terminal was started by HerdR Terminal |
| `herdr.terminal.space=<id>` | TUI space id |
| `herdr.terminal.tab=<id>` | TUI tab id |
| `herdr.seen` | Shared cross-client convention; last turn seen |
| `herdr.pinned` | Shared cross-client convention; pinned in the UI |

Terminals are started with `TERM=xterm-256color`, `COLORTERM=truecolor` and `TERM_PROGRAM=herdr` (the Herdr family marker). On restart, list terminals, match the TUI labels, reattach; never spawn duplicates.

### 4.6 Core client and lifecycle

**Startup**

1. Discover the core binary (configured path, `PATH`, login shell `PATH`, usual install dirs; accept it only if `--version` prints `hc …`).
2. Connect to the socket or `hc connect`; send `hello {"framed": true}`.
3. Check generation, version, methods and features; fail with a clear message if a required method is missing.
4. `events.subscribe`; keep the snapshot.
5. Read the `herdr-terminal` document; reconcile it with the snapshot and labels.
6. Attach the visible terminals; start missing shells only when the user asks.

**Reconnect**

- A dropped connection does **not** mean the programs died.
- Reconnect, `hello`, resubscribe; on `missed`, resnapshot; re-read documents; reattach terminals.
- On `lost`, the kept terminal stays live; wait for `restarted` and attach again.
- On `finished`, remove the stream but keep the record/scrollback as configured.

**Shutdown**

- Detach streams (CLOSE), restore the terminal, flush the document; do not stop core unless the user explicitly asks.

## 5. Feature scope

Statuses:

- **v1** — required for the first usable release.
- **Later** — planned, with a phase.
- **Postponed** — deliberately deferred; has a trigger.
- **No** — not planned; reason given.

### 5.1 Supported in v1

| Area | Feature |
|---|---|
| Connection | Core discovery, long-lived framed connection, `hello` check, reconnect |
| Terminals | Run, attach, render, keys, mouse wheel/click, resize, focus, FACTS |
| Workspace | Spaces, tabs, panes, split/close/move, focus, zoom |
| Terminals | New/close/rename, labels, keep, kill/rm, shell/command |
| Agents | Status, turn, title, foreground, list/sidebar, peek, seen/pinned |
| UI | Tab bar, pane borders, status bar, command palette, help, toast/confirm/prompt |
| Persistence | Own `herdr-terminal` document, restore, multi-screen CAS |
| Themes | 16/256/truecolor, default palette, basic theme config |
| Recovery | `lost`/`restarted`/`finished`, `missed`, heartbeat, reattach |
| Diagnostics | Version, core status, logs, `ht status` |

### 5.2 Expected later

| Phase | Feature | Notes |
|---|---|---|
| 1 | Layout persistence and restore | Space/tab/pane tree in the document |
| 1 | Resize, drag, tab reorder | Mouse and keyboard |
| 2 | Agent sidebar, peek, notifications | Status/turn/title/foreground from core |
| 2 | Command palette, keybindings, help, settings | Native widgets |
| 3 | Machines: add/connect/list/rename/rm | Remote terminals and events |
| 3 | Files: read/write/edit, diff, external editor | Core `file.*`; no embedded editor |
| 3 | File picker | Simple list; use `machine.exec`/`file.*` |
| 4 | Scrollback search, copy/paste, clipboard | Terminal-dependent |
| 4 | Custom layouts, themes, accessibility | Settings and config |
| 4 | Packaging, install, `--skill`, CLI docs | Distribution |
| 5 | Mods/plugins (manifest + external commands) | See §6.3 |
| 5 | Kitty graphics/images | See §6.3 |

### 5.3 Postponed

| Feature | Why postponed | Trigger / phase |
|---|---|---|
| Mods/plugins | Needs a plugin model and a host API; not required for v1 | After Phase 4; use Herdr 1.x's manifest + external-command model, not JS/GPUI |
| Kitty graphics / images | Terminal support varies; VT binding work | Phase 5, only if users need it |
| Embedded code editor | Large scope; cell-grid editor is a product of its own | Use external `$EDITOR`; revisit only if demanded |
| Markdown rendering | Rich text is out of scope for a TUI | Use an external pager/`glow`/`bat` |
| Advanced settings UI | Basic config first | Phase 4; simple forms |
| Drag-and-drop | Mouse support is terminal-dependent | Later; keyboard/mouse alternatives first |
| Shared workspace with Herdr App | Couples two products' schemas | Separate design; not default |
| Native OS notifications | Terminal-dependent | Later; bell/OSC fallback first |

### 5.4 Not supported

| Feature | Reason |
|---|---|
| Full code editor | TUI cell grid; editor is a separate product; use `$EDITOR` |
| Markdown/rich text | Terminal text model; use an external viewer |
| SVG/images (without a graphics protocol) | Terminal support varies; external viewer |
| Animations/transitions | A TUI redraws cells; no animation framework; spinners only |
| OS windows, floats, transparency | The terminal emulator owns windows; use panes/popups |
| Pixel styling, shadows, rounded corners | Cell grid; terminal themes control colors |
| Font selection | Terminal emulator controls fonts |
| GPUI/JS mods | Runtime is very large; mods describe GPUI elements, not cells |
| Screen-based agent detection | Forbidden: status is only OSC 7501 |
| Herdr App's `herdr-app` document | App-private; separate product |
| Herdr App's `herdr.space` / `herdr.tab` labels | App-private; would make App adopt TUI terminals |
| Multi-window | One terminal window; use spaces/tabs/panes |
| Telemetry/update UI | Not planned; package manager/core update handles it |
| Win98 demo mod | Demo of GPUI customization; not a TUI feature |

## 6. Phases

### Phase 0 — Foundation

**Goal:** prove the hard path: connect, attach, render, input, reconnect.

- Core discovery and long-lived framed connection.
- `hello` feature check.
- `events.subscribe` snapshot.
- Run/attach one terminal.
- Local VT, cell rendering, cursor, wide characters.
- Key encoding, RESIZE, FOCUS, FACTS, credit.
- Reconnect and reattach; `lost`/`restarted`/`finished`; `missed`.

**Done when:** a shell, vim and htop work in one pane; resize works; core restart reconnects; no leaked threads or streams.

### Phase 1 — Workspace

**Goal:** make it a daily terminal workspace.

- Spaces, tabs, panes; split/close/move; focus; zoom.
- Tab bar, pane frames, status bar.
- New/close/rename; labels; keep; kill/rm.
- `herdr-terminal` document; restore; multi-screen CAS.
- Terminal rediscovery by labels.

**Done when:** a multi-pane layout survives restart; two screens share the layout; no duplicate terminals.

### Phase 2 — Agents and awareness

**Goal:** make agents visible and steerable.

- Status, turn, title, foreground.
- Agent list/sidebar; peek; seen/pinned labels.
- Notifications for blocked/error/finished.
- Command palette, keybindings, help, confirm/prompt/toast.
- Basic settings.

**Done when:** an agent's state changes are visible without screen scraping; the user can jump to it, mark it seen and pin it.

### Phase 3 — Machines and files

**Goal:** one workspace across machines.

- Machine add/connect/list/rename/rm.
- Remote terminals, events and documents.
- Reconnect after network loss.
- File read/write/edit; diff; external editor; simple file picker.

**Done when:** a remote terminal and a local terminal live in the same workspace; a remote file can be inspected/edited; reconnect restores the view.

### Phase 4 — Polish and distribution

**Goal:** make it a daily driver.

- Mouse resize/drag; scrollback search; copy/paste and clipboard.
- Themes, custom layouts, accessibility.
- Performance: redraw coalescing, large output, many panes.
- Packaging, install, `--version`, `--skill`, CLI/docs, CI.
- End-to-end tests and golden buffers.

**Done when:** a full day of work is comfortable; install and upgrade are documented; tests run in seconds.

### Phase 5 — Later

- Mods/plugins using the manifest + external-command model.
- Kitty graphics/images if there is demand.
- Shared workspace schema with Herdr App only if deliberately designed.
- Advanced settings and theming.

## 7. Testing and conventions

- **Toolchain:** Rust 1.95.0, `just`; Zig 0.16.0 only for the libghostty-vt binding.
- **Stack:** ratatui 0.30, crossterm 0.29, libghostty-vt, unicode-width/segmentation.
- **TDD:** one good test per documented behavior; pure model/layout/command tests; protocol tests; end-to-end with real PTYs where it matters.
- **Isolation:** tests use a unique `HC_SESSION` and scratch XDG paths under `/var/tmp`; never point tests at a real running core.
- **No flaky sleeps:** wait on events/state, not timers.
- **Rendering tests:** compare cell buffers (golden), not screenshots.
- **File size:** keep files under ~32 KB where practical; split along real seams.
- **Source rules:** do not copy Herdr 1.x code; use it only for ideas and edge cases.
- **Commits:** lowercase conventional commits, single line, no AI co-author; work on a branch and open a PR; only the maintainer releases.
- **Release notes:** written at release time in `CHANGELOG.md`; normal work does not edit it.
- **Core changes:** new methods/fields are separate work in `herdr-core` under its rules; feature-gate on `hello`.

## 8. Open questions

1. **VT binding:** extract a shared `ghostty-vt` crate, reuse Herdr App's `ghostty-sys`, or create a TUI-local binding? The TUI needs styled cells and key/mouse encoding, which core's minimal binding does not expose. A pure-Rust VT (for example `alacritty_terminal`) would remove the Zig build dependency but diverge from App/Core behavior; decide before Phase 0.
2. **Async runtime:** threads + channels vs tokio. Start simple; measure.
3. **Document layout:** one `herdr-terminal` document per core with machine-qualified ids, or one document per machine like Herdr App? Start local, revisit for Phase 3.
4. **Clipboard:** OSC 52 vs native clipboard vs both; terminal-dependent.
5. **Notifications:** bell/OSC 9/777 vs platform notifications; what is portable?
6. **Kitty graphics:** is it a real requirement or a demo feature?
7. **Mods:** when, and how closely to follow Herdr 1.x's manifest? v1 has none.
8. **Name styling:** product name is **HerdR Terminal** (capital R); code identifiers are `herdr-terminal`. Confirm the stylization before public docs.
9. **Shared workspace with Herdr App:** never by default; if ever, design a common versioned schema on purpose.
10. **Core changes:** none expected for v1; if needed, separate work in `herdr-core`.

## 9. References

- [Core and App Reference](01-core-and-app-reference.md)
- **Herdr Core** (repository: `herdr-core`): the schema README for protocol, attach, events, documents and status; `AGENTS.md` for core rules and traps.
- **Herdr App** (repository: `herdr-app`): `AGENTS.md`; `hc.rs`, `core_bin.rs` for connection and discovery; `model.rs`, `layout.rs` for workspace/layout; `mirror.rs`, `mirror_sync.rs` for document CAS; `terminal.rs`, `vt.rs` for attach, VT and input; the defaults `AGENTS.md` for the app interface.
- **Herdr 1.x** (repository: `herdr`): the plugin documentation, as a reference for the future mod model.
