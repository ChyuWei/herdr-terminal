# Core and App Reference

**Status:** draft, 2026-11-02
**Audience:** contributors to HerdR Terminal.
**Purpose:** describe what Herdr Core and Herdr App each own, how they talk, and what the new TUI can reuse. This is a reference, not the TUI design; see [02-architecture.md](02-architecture.md).

## 1. The family

| Program | Binary | Role | Status |
|---|---|---|---|
| Herdr Core | `herdr-core` / `hc` | Runtime: PTYs, terminal lifetime, raw output relay, attach, events, status, documents, machines, exec/file | Active |
| Herdr App | `herdr-app` | Desktop client (GPUI): spaces/tabs/panes, agents, mods, settings | Active |
| Herdr TUI (1.x) | `herdr` | Old monolithic TUI: its own server, protocol, persistence and screen-based detection | Reference only; do not modify, build, link or copy |
| HerdR Terminal | `herdr-terminal` / `ht` | New TUI client of Herdr Core | This project |

The important shift: Herdr 1.x was a monolith with its own server. HerdR Terminal is **a client of Herdr Core**, exactly like Herdr App. It does not embed a server, a PTY manager, a wire protocol, or agent detection.

## 2. Herdr Core: a runtime, not a multiplexer

Core owns the programs and the facts about them. It deliberately has **no concept of spaces, tabs, panes or layouts**; those belong to the client.

### 2.1 What Core owns

| Concern | Detail |
|---|---|
| PTYs and processes | Spawns programs, keeps terminals alive, records exit code/signal/reason |
| Terminal records | Address, name, labels, argv, cwd, state, pid, size, title, status, turns, foreground, parent/children |
| Raw output | Relays bytes; sends a first paint on attach; never sends draw commands |
| Attach streams | One channel per viewer; credit-based flow control; resize/focus/display facts |
| Events | Snapshot plus `event` notifications, `heartbeat`, `missed` |
| Status | Only what programs report through OSC 7501; never inferred from the screen |
| Documents | Opaque, versioned JSON up to 1 MiB; core never interprets it |
| Machines | Adds/connects remote cores; merges their terminals into one event stream |
| Exec/files | `machine.exec`, `file.read`, `file.write`, `file.edit` |
| Templates | Ready-made agent roles (`templates.list`) |
| Server | `server.status`, `server.stop`, `server.restart` |

### 2.2 Connection modes

- **Plain mode:** JSON-RPC 2.0, one message per line. Any language or `socat` works.
- **Framed mode:** `hello` with `{"framed": true}` switches after its reply. Attach needs it.
- A client keeps **one long-lived framed connection**. Do not spawn `hc` per user action.
- Local clients can connect to the core socket directly; `hc connect` speaks the same protocol over stdin/stdout and crosses SSH.

Framed frame: `[channel u32 LE][kind u8][len u32 LE][payload]`.

| Kind | Name | Meaning |
|---|---|---|
| 0 | DATA | Channel 0: one JSON-RPC message; other channels: terminal stream bytes |
| 1 | CREDIT | Bytes the receiver consumed |
| 2 | CLOSE | Channel ended; from core, a JSON reason |
| 3 | RESIZE | Viewer → core: `u16 cols, u16 rows` |
| 4 | FOCUS | Viewer → core: 1 gained, 0 lost |
| 5 | FACTS | Viewer → core: JSON display facts (`fg`, `bg`, `cursor`, `cell`) |

### 2.3 Attach streams

`terminal.attach` returns a channel. Core sends:

1. the **first paint** (current screen, optional history, latest status report and title, and the win32-input-mode state);
2. the program's raw output, minus queries core already answered.

The viewer sends keys as DATA, plus RESIZE, FOCUS and FACTS as they change. Core never encodes keys. A stream starts with 256 KiB of credit; a viewer more than 4 MiB behind gets a fresh repaint. The last active viewer sets the terminal size.

### 2.4 Events

`events.subscribe` returns a snapshot (`seq`, `terminals`) and, in framed mode, a channel. Then come `event` notifications, a heartbeat every 15 s, and `missed` plus a fresh snapshot when a subscriber falls behind.

| Type | Data | Apply |
|---|---|---|
| `created` | whole `Terminal` record | add it |
| `lost` | `{reason, exit_code, signal, finished_ms: null}` | kept terminal remains live; set these, clear pid/foreground |
| `restarted` | whole `Terminal` record | replace it; reattach |
| `keep` | `{keep}` | set `keep` |
| `finished` | `{reason, exit_code, signal, finished_ms}` | set finished; drop foreground |
| `removed` | `{}` | drop it |
| `status` | `{status, records, reported}` | replace all three |
| `turn` | `{turn, state}` | set `turn` |
| `title` | `{title}` | set `title` |
| `labels` | `{labels, record?}` | replace labels; add `record` if newly matching |
| `size` | `{cols, rows}` | set both |
| `foreground` | `{pid, name, argv, cwd}` or null | set foreground |
| `doc` | `{name, version, machine?}` | read `doc.get`; the event has no value |

`seq` is per core process. After reconnecting, take a fresh snapshot. A `missed` notification means: drop events whose `seq` is not above the snapshot's, then re-read documents.

### 2.5 Documents

`doc.get {name, on?}` returns `{value, version}`; an unwritten name is `{value: null, version: 0}`. `doc.set {name, value, version, on?}` writes only if the version matches and returns the new version. A stale write fails with kind `conflict`. Writing null deletes the value but keeps the version.

Documents are opaque to core, persist in core state, survive restart and handover, and are limited to 1 MiB of serialized JSON. A `doc` event carries only the name and version; call `doc.get` to read the value. Subscribe before reading; snapshots contain terminals only, not documents.

### 2.6 Status

Status is **only** the Program Status Protocol, OSC 7501: `idle`, `working`, `done`, `blocked`, `error`, and whatever else a program reports. Core stores it as reported. Programs that do not speak 7501 have no status. Never infer status from screen text, and never add per-agent or per-vendor detection.

### 2.7 API generation

The API is defined once in Herdr Core's `types.rs` plus the method table in `api.rs`; those definitions also generate the CLI and `hc.openrpc.json`. Protocol generation 1 only grows; never break an existing client. `hello` reports generation, version, available methods and features.

Core methods, grouped:

| Area | Methods |
|---|---|
| Discovery | `hello`, `rpc.discover`, `templates.list` |
| Server | `server.status`, `server.stop`, `server.restart` |
| Terminals | `terminal.run`, `list`, `get`, `kill`, `rm`, `keep`, `label`, `read`, `attach`, `send`, `wait`, `keys`, `interrupt`, `put` |
| Machines | `machine.add`, `list`, `connect`, `rename`, `rm`, `exec` |
| Files | `file.read`, `file.write`, `file.edit` |
| Documents | `doc.get`, `doc.set` |
| Events | `events.subscribe` |

## 3. Herdr App: the reference desktop client

Herdr App is a GPUI desktop application and the reference implementation of a UI on core. It is **not** a library and not a dependency of the TUI.

### 3.1 What App owns

| Concern | Detail |
|---|---|
| Workspace | Spaces, tabs, panes, split trees, zoom, focus, layouts |
| Agents UI | Status, turns, titles, foreground, sidebar, peek, seen/pinned, notifications |
| Rendering | GPUI widgets plus libghostty-vt for terminal cells |
| Mods | Built-in and third-party mods in JavaScript on GPUI (`gpui-kit`, `gpui-shell`, QuickJS) |
| Settings | Themes, fonts, keybindings, programs, settings, layouts |
| Files/changes | Code editor, diff view, worktree, machine add |
| CLI | Commands, `--skill`, `status`, docs |
| Shared layout | Document `herdr-app`, versioned CAS per machine |
| Local view | Focus, zoom, selection and similar per-screen state in `view.json` |

### 3.2 How App talks to Core

1. Discover the core binary (configured path, `PATH`, login shell `PATH`, usual install dirs; accept it only if `--version` prints `hc …`).
2. Start `hc connect` and speak framed JSON-RPC over its pipes.
3. `hello {"framed": true}`; check generation, version, methods and features.
4. `events.subscribe`; keep the snapshot and apply events.
5. `terminal.run` with `TERM=xterm-256color`, `COLORTERM=truecolor`, `TERM_PROGRAM=herdr`, and namespaced labels.
6. `terminal.attach` per visible terminal; feed raw bytes to a local libghostty-vt; send keys as DATA; send RESIZE/FOCUS/FACTS.
7. Read/write the `herdr-app` document with `doc.get`/`doc.set` and optimistic-concurrency retries.
8. On `lost`/`restarted`, reattach; on `finished`, remove the stream; on `missed`, resnapshot; on a dropped connection, reconnect and resubscribe.

### 3.3 App's labels

| Label | Owner | Meaning |
|---|---|---|
| `herdr.space` | App | App workspace attachment |
| `herdr.tab` | App | App tab attachment |
| `herdr.right-click` | App | App-private UI state |
| `herdr.seen` | Shared convention | Last turn the user has seen; written by clients that show turns |
| `herdr.pinned` | Shared convention | Terminal is pinned in the client's UI |

Herdr App's labels document explicitly anticipates other Herdr clients, including a terminal UI. The App's `herdr.space`/`herdr.tab` are **App-private**: a new client must not write them or interpret them as its own workspace. The cross-client `herdr.seen`/`herdr.pinned` are the exception; a TUI should follow the same rules if it shows seen/pinned state.

## 4. Boundary and ownership

| Concern | Herdr Core | Herdr App | HerdR Terminal |
|---|---|---|---|
| PTY, process lifetime, raw output | owns | uses | uses |
| Attach, flow control, first paint | owns | uses | uses |
| Terminal records, status, turns, foreground | owns/stores | reads | reads |
| Events and snapshots | owns | subscribes | subscribes |
| Documents | stores opaque JSON | owns `herdr-app` | owns `herdr-terminal` |
| Spaces/tabs/panes/layout | none | owns | owns |
| Focus, zoom, selection | none | local | local |
| Labels | stores | writes `herdr.*` (App-private) | writes `herdr.terminal.*` + shared `herdr.seen`/`herdr.pinned` |
| Mods | none | JS/GPUI | none in v1; later manifest + external commands |
| Machines | owns connections | UI | UI |
| Rendering | none | GPUI + libghostty-vt | ratatui + libghostty-vt |
| Key/mouse encoding | none | local VT | local VT |

Rules that follow from the boundary:

- Core is a runtime, not a multiplexer. Do not move spaces/tabs/panes/layout into core.
- Core never sends draw commands. The client runs its own VT and composes its own screen.
- Status is only OSC 7501. No screen scraping, no agent registry, no vendor-specific logic.
- A client owns its document and its label namespace. Do not read another client's private document or labels as if they were yours.
- The API only grows. Feature-gate on `hello` methods/features rather than assuming a version.

## 5. What the TUI can reuse

Reuse the **protocol and the patterns**, not the App's UI stack:

- Core's framed JSON-RPC, attach streams, events and documents.
- The design of Herdr App's `hc.rs` for a long-lived framed client.
- Herdr App's `model.rs` and `layout.rs` as references for a pure workspace/layout model.
- Herdr App's `mirror.rs` and `mirror_sync.rs` as references for CAS document sync.
- Herdr App's `terminal.rs` and `vt.rs` as references for attach, local VT, FACTS and key encoding.
- Herdr App's `commands.rs` as a reference for a single command table used by keys, palette and CLI.
- libghostty-vt, via a binding.
- The cross-client `herdr.seen` and `herdr.pinned` conventions.
- The feature set of App's built-in mods, reimplemented as native TUI panels.

## 6. What the TUI must not reuse

- **Herdr 1.x code** (the `herdr` repository): no server, protocol, persistence, screen detection or rendering code. It is reference for ideas and edge cases only.
- **App's `herdr-app` document schema**: it is App-private. The TUI uses `herdr-terminal`.
- **App's `herdr.space` / `herdr.tab` labels**: writing them would make the App adopt TUI terminals into its spaces.
- **GPUI/JS mods and the `gpui-kit` runtime**: too large, and existing mods describe GPUI elements, not a cell grid.
- **GPUI widgets, pixel styling, OS windows, animations**: no cell-grid equivalent.
- **Screen-based agent detection**: forbidden by core's design.

## 7. Capability checklist for any client

This checklist is a generic capability reference for any client, not the
implementation status of this repository; current status is in
[02-architecture.md §5.5](02-architecture.md#55-implemented). A client that
implements Herdr's core workflow needs:

- [ ] Discover and connect to core; check `hello`.
- [ ] Subscribe to events; keep a snapshot; handle `missed`; resnapshot.
- [ ] Run terminals with `TERM`/`COLORTERM`/`TERM_PROGRAM` and namespaced labels.
- [ ] Attach, render raw output through a local VT, send keys/resize/focus/facts.
- [ ] Keep a long-lived connection; reconnect without assuming programs died.
- [ ] Own a workspace model and persist it in a namespaced document.
- [ ] Rediscover its own terminals after a restart by labels.
- [ ] Show status, turns, titles and foreground from core; never infer them.
- [ ] Handle `lost`, `restarted`, `finished`, `removed`, `keep` and `missed`.
- [ ] Feature-gate on `hello` methods/features.
- [ ] Use isolated `HC_SESSION` and scratch state paths in tests; never touch a real core.

## 8. Reading list

**Herdr Core** (repository: `herdr-core`)
- The schema README — connection modes, attach streams, events, documents, status.
- `AGENTS.md` — core's rules and traps.
- `types.rs`, `api.rs` — API definitions and method table.
- `terminals.rs`, `viewers.rs` — terminal records and display facts.

**Herdr App** (repository: `herdr-app`)
- `AGENTS.md` — app rules, platform guidance, sandbox.
- `hc.rs` — framed client over `hc connect`.
- `core_bin.rs` — core discovery.
- `model.rs`, `layout.rs` — workspace and layout models.
- `mirror.rs`, `mirror_sync.rs` — shared layout document and CAS sync.
- `terminal.rs`, `vt.rs` — attach, local VT, rendering, input.
- `commands.rs` — command table.
- The labels document — cross-client label rules.
- The defaults `AGENTS.md` — the App's interface as files.
- `herdr.d.ts` — mod API reference.

## 9. Glossary

| Term | Meaning |
|---|---|
| Core | The `herdr-core` runtime process (`hc`) |
| Client | A UI that connects to core: Herdr App, HerdR Terminal, future clients |
| Attach | A terminal stream channel: first paint, raw output, input, resize/focus/facts |
| First paint | The current screen and optional history sent when a viewer attaches |
| Credit | Bytes a viewer has consumed; flow control for attach streams |
| FACTS | Viewer display facts: foreground/background colors, cursor, cell size |
| Document | Opaque versioned JSON stored by core; clients own their names |
| Label | Key/value metadata on a terminal, stored by core |
| Status | OSC 7501 report from the program; never inferred |
| Turn | A counter after an agent turn ends |
| OSC 7501 | The Program Status Protocol |
| Generation | Core's protocol generation; generation 1 only grows |
