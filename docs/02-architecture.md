# HerdR Terminal — Architecture

**Status:** Architecture; current implementation status in §5.5
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
- **Shared layout, local UI.** The `herdr-terminal` document holds layout; focus, zoom and the sidebar flag stay in the per-screen view; scrollback, search, selection and theme stay in memory.
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
| `main`, `lib` | CLI entry, terminal setup/restore, event loop, shutdown |
| `core` | Core discovery, framed connection, calls, attach streams, events, reconnect |
| `model` | Pure event and terminal model shared by the screen and workspace |
| `screen` | Single-pane lifecycle, attach playback, pointer and view |
| `vt` | libghostty-vt binding: cells and grapheme clusters, keys, mouse, scrollback, search, selection |
| `workspace` | Workspace model, split tree and geometry, commands, keybindings, palette, agents, startup, sync, ratatui UI; `runtime/` holds the event loop, attach, input, mouse, overlays, reconnect and agent handling |
| `workspace::state`, `workspace::local` | Runtime UI state and per-screen view |
| `config` | Startup settings: theme, scrollback, mouse, clipboard, prefix keys |
| `theme` | Chrome colors, default/dark/light, 16/256/truecolor mapping |
| `clipboard` | OSC 52 encoding and native helper fallback |
| `tui`, `term`, `session`, `probe` | Terminal backend, process/session ownership, `ht screen` and `ht probe` |

The workspace model, geometry, commands, keys, palette and agents are pure and
unit-testable without a PTY. IO lives in `core`, `screen`, `vt`,
`workspace/runtime`, `tui` and `main`. `ht` has no `status`, `commands` or
`--skill` subcommand yet; those remain planned.

### 4.3 Rendering pipeline

1. The attach stream delivers the **first paint** and then raw program bytes.
2. Feed bytes into the local VT; it owns screen, scrollback, cursor, modes, title and responses.
3. On redraw, update the VT render state and iterate visible rows/cells.
4. Map each cell's symbol, style, foreground/background and attributes into a ratatui `Buffer` cell; honor wide cells and continuation cells.
5. Compose the TUI around the terminal area: tab bar, pane borders, status bar, sidebar, overlays.
6. Diff and present; use synchronized output when the terminal supports it; place the cursor only for the focused terminal.
7. Send **FACTS** when colors or cell size change, **RESIZE** when a pane changes size, **FOCUS** when focus changes.

The TUI owns the screen. It does not ask core for frames or scrape `terminal.read`.

Grapheme clustering (DEC mode 2027) is a VT reset default, matching Core and
Herdr App; it is client-local and needs no Core version gate. A
variation-selector emoji such as `✍️` or `❤️` is one cluster two cells wide,
followed by a zero-width spacer cell, not a base cell plus a separate
combining cell, and RIS keeps the default. This keeps cell width, cursor
placement, selection and search consistent with Core.

### 4.4 Input

- crossterm events: key, mouse, paste, focus, resize.
- Enable keyboard enhancement flags and bracketed paste; restore on exit.
- Encode keys through the local VT so kitty keyboard protocol and Windows win32-input-mode are honored.
- Send keys as **DATA** on the terminal's attach channel.

**Mouse precedence**, in order:

1. An in-progress split resize or selection drag owns every mouse event.
2. An open overlay (help, prompt, palette, search, peek, sidebar) consumes
   every mouse event. A selection is not in that list: a fresh press leaves
   selection and is routed as if none existed.
3. Chrome: a tab-bar click selects a tab, a status-bar click is consumed, and
   a split-gap press starts a resize. Chrome never reaches a program.
4. A pane-grid press focuses the pane; a border press only focuses it. With
   Shift, or when the program does not track the mouse, the press arms a local
   selection whose release stays local even without a drag. Selection starts
   only when the pointer reaches another cell, so press-and-release in one cell
   is a plain focus click (with Shift, a local click that sends nothing).
5. Without Shift, a tracking program receives press, drag and release in
   pane-local coordinates. Shift-drag selects locally instead.
6. The wheel is forwarded only to a tracking program with no active selection;
   otherwise it scrolls that pane's local history. A wheel over chrome is
   consumed.

RESIZE is sent only when a pane's grid actually changes. Overlays and chrome
never change pane geometry.

**Selection, copy and paste**

- `prefix [` starts a keyboard selection at the focused cursor; h/j/k/l or the
  arrows extend it, `y` or Enter copies, Esc cancels. Mouse click-drag selects
  on a non-tracking grid; Shift-drag selects over a tracking program. Selection
  is cell-based; word and line selection are not provided.
- Copied text strips cell padding: wide cells are copied once and row ends are
  trimmed. An empty selection copies nothing.
- Copy uses OSC 52 by default. `clipboard` selects OSC 52, a native helper
  (`pbcopy` on macOS, `wl-copy`/`xclip` on Linux), both, or off; a native
  failure falls back to OSC 52.
- Any other key leaves selection, snaps the pane to the live bottom and reaches
  the focused program, so selection never traps typing. A forwarded key also
  forwards its release when the program requested kitty key events.
- Paste goes only to the focused program when no overlay or selection is open.
  It is framed with bracketed paste only when the program enabled DEC 2004.

### 4.5 State and persistence

**Shared workspace** is stored in a document named `herdr-terminal`, schema
version 1:

```json
{"version": 1,
 "spaces": [{"id": "4294967297", "name": "space 1", "folder": "",
             "tabs": [{"id": "4294967298", "name": "tab 1",
                       "layout": {"leaf": {"id": "4294967299", "address": "t7",
                                           "name": "t7", "labels": {}}}}]}]}
```

- Ids are decimal strings (u64), never reused. The default screen allocates in
  namespace 1; every other screen id gets an FNV-1a-32 namespace, so two
  screens cannot mint the same id.
- A layout is `{leaf: …}` or
  `{axis: "columns"|"rows", children: [{size, layout}]}`; a tab layout of
  `null` is the empty tab.
- Leaves reference terminal addresses and the labels the terminal carries.
  Focus, zoom, history and the active space/tab are deliberately absent.
- Writes are compare-and-set via `doc.get`/`doc.set`. A `conflict` re-reads,
  merges and retries: the merge is per space and per tab. One-sided changes
  win, a deletion against unchanged content applies, additions from both
  screens are kept. When both screens changed the same tab the local tab wins
  and the conflict is reported; the other screen's terminal is then recovered
  from its labels instead of being lost.
- Subscribers read the document after subscribing, on a `doc` event, after
  `missed`, and after reconnecting. Documents are not in snapshots and changes
  while disconnected are not replayed.
- Writes are bounded (a 2 s call timeout, at most three CAS rounds per loop);
  a failure leaves the document dirty and retries, it never spins the UI.

**Local view** is per screen and stays on the machine:
`$XDG_STATE_HOME/herdr-terminal/view-<screen>.json` (falling back to
`$HOME/.local/state`). It holds the active space/tab, focus, zoom, focus
history and whether the agent sidebar is open. The screen id comes from
`HERDR_TERMINAL_SCREEN` (1–64 characters of
`[A-Za-z0-9._-]`, default `default`). A missing or malformed view is a warning,
not an error; ids that no longer exist are ignored. Scrollback, search,
selection and theme are runtime-only: they are not stored here or in the
shared document.

**Labels** identify the TUI's own terminals after a restart:

| Label | Meaning |
|---|---|
| `herdr.client=herdr-terminal` | Terminal was started by HerdR Terminal |
| `herdr.terminal.space=<id>` | TUI space id |
| `herdr.terminal.tab=<id>` | TUI tab id |
| `herdr.terminal.pane=<id>` | TUI pane id, carried before `terminal.run` returns |
| `herdr.seen` | Shared cross-client convention; last turn seen |
| `herdr.pinned` | Shared cross-client convention; pinned in the UI |

Terminals are started with `TERM=xterm-256color`, `COLORTERM=truecolor` and
`TERM_PROGRAM=herdr` (the Herdr family marker). On startup, list terminals,
match the document's panes by exact address and pane label (falling back to a
unique pane label when a terminal was rediscovered under a new address),
reattach them, and never spawn duplicates. An address named by the document
but missing from the subscription snapshot is looked up with `terminal.get`
before it may be pruned, so a pane created by another screen just after the
snapshot is still restored. A pane whose terminal is confirmed gone is pruned
from the document; the tab stays. A deleted document is rebuilt from the
labels of live terminals at its retained version instead of being replaced by
a new shell. An explicitly given PROGRAM starts in a new tab (or the empty
tab), not in place of a restored pane. Restored and
recovered panes are workspace-owned for close semantics (`x` ends a non-kept
program, `e` detaches, `X` kills), while their `Session` is adopted so a local
startup or fatal error never kills a program this process did not start.

### 4.6 Agents, attention and the sidebar

An **agent** is a terminal in this workspace whose core record is `reported`
or has a current `status`; both come only from OSC 7501. The client never
infers agency or status from screen text, titles, argv, process names or a
vendor registry. Each row derives a word from core's status:

| Word | Meaning |
|---|---|
| `blocked` | reported `blocked` or `error` |
| `working` | reported `working` |
| `done` | reported `done`, or no status on an unseen turn |
| `idle` | no status on a seen turn |
| other | the program's own state string |

A turn is unseen when its number is greater than `herdr.seen`; a terminal
without a valid label falls back to a transient local flag that is set by
looking and cleared by a hidden busy→non-busy transition. Nothing local is
persisted.

**Sidebar** (`a`) lists every agent across all spaces and tabs. Pinned agents
first, then blocked > done > working > idle (unknown sorts with idle), then
workspace order and address. `j`/`k` move, `g`/`G` go to the first/last row,
`Enter` jumps, `i` peeks, `p` toggles the pin, `u` marks unread; `Esc` or `a`
closes. Selection is by pane id and follows reordering. Only the open flag is
local view state (`sidebar_open`, schema v1); the selection is not persisted.

**Peek** (`prefix i`, or `i` on a sidebar row) draws the pane's existing local
VT plus core facts (title, address, word, turn, pin, app, progress, kind,
message). It never calls `terminal.read` and never infers status from the
text. Any key closes it; `Ctrl-]` still detaches.

**Jump** (`prefix g`/`G`) cycles agents in sidebar order, clears zoom, and does
not write the document.

**Attention**: after a baseline built from the first snapshot, a newly
`blocked` row and a hidden busy→non-busy transition produce one in-app
status-bar notice and one BEL; no OS notification. Looking at a terminal
suppresses the "finished" alert. A `missed`/reconnect rebuilds the baseline so
old transitions are not replayed.

**Seen and pin** are shared core labels, raised through `terminal.label`:
`herdr.seen` is written once per visible or peeked `(address, turn)` and only
raised (a missing label is written with the current turn, including 0). `u`
writes `turn - 1`, the only lower write. `herdr.pinned` is `"1"` or removed.
Writes are optimistic, use core's returned record as authoritative, give a
failed seen write at most three attempts in total per turn (the first try and
two retries), and are dropped when the turn changes, the pane disappears or a
`labels` event confirms the value. New
terminals start with `herdr.seen=0`. These labels are deliberately excluded
from document capture and from restore's repair comparison: seen/pin changes
must never dirty the layout document.

### 4.7 Core client and lifecycle

**Supported Core**

Core 0.1.16 or newer at protocol generation 1. Optional capabilities are gated
on `hello.features`, never a version string. Testing uses 0.1.17; 0.1.16 is
checked by source and the no-feature path, not by running a 0.1.16 binary.

**Shared Core**

A user has one Core per `HC_SESSION`. Its socket resolves the same way for
every client: `HC_SOCKET`, the runtime directory under `XDG_RUNTIME_DIR`, or the
core config's `socket`; when no Core is running, clients should resolve the same
`hc` binary. A running compatible Core is shared by Herdr App, `ht`, multiple
`ht` screens and CLI calls. `ht` connects with `hc connect` and never starts a
private Core per action. Herdr App's update path runs `hc update`, which swaps
the binary and restarts the Core while terminals keep running; `ht` reconnects
and reattaches. `ht` has no update command.

**Startup**

1. Discover the core binary (configured path, `PATH`, login shell `PATH`, usual install dirs; accept it only if `--version` prints `hc …`).
2. Connect to the socket or `hc connect`; send `hello {"framed": true}`.
3. Check generation, version, methods and features; fail with a clear message if a required method is missing.
4. `events.subscribe`; keep the snapshot.
5. Read the `herdr-terminal` document. If it is absent (version 0), claim an
   empty one at version 0; a lost claim race re-reads the winner. If it was
   deleted (value null, retained version > 0), recover the live labeled
   terminals first, apply the local view, and recreate the document at the
   retained version without starting a shell. Only when no labeled terminal
   survives does this screen claim an empty document and run the initial
   shell, and only if it won that claim.
6. Reconcile an existing document with the snapshot and labels: the snapshot
   is not authoritative for addresses the document names, so every such
   address missing from the snapshot is checked with `terminal.get` first. A
   live record is adopted and must not be pruned; a `not_found` may be pruned.
   Recover labeled orphans, apply this screen's local view, and never replace
   a documented pane. Start a shell only for a document this screen created,
   a won deleted-document claim with no recovered pane, or an explicit
   PROGRAM.

**Initial size**

Every spawn path sends `terminal.run`'s optional `size` when it knows the grid
the terminal will attach at: the initial shell, a new pane/split/tab/space, an
explicit PROGRAM after restore, `ht screen`, a replacement after reconnect, and
`ht probe`. Workspace starts use the pending pane's exact inner grid, previewed
on a clone of the deterministic insertion so a failed run leaves no empty tab.
A path that cannot know the size omits the field and core's default applies.
Older cores ignore the unknown field; no version gate is used. This matters
because a program that reads its size once at startup, before a viewer
attaches, would otherwise draw its first frame at core's default size.

**Observer attach**

The client library can attach a terminal as a read-only observer for a future
peek view; no peek UI uses it yet. `terminal.attach` takes `observe` and
`resize_notifications`; the client requests them only when `hello.features`
advertises `resize_notifications`, and no Core version string is consulted. The
reply must confirm the feature, or the client abandons the channel and fails
instead of falling back to a normal attach that would take over the grid. A
confirmed observer receives RESIZE (kind 3, four little-endian bytes) before
each paint that uses a new grid, applies it to the local VT before the following
DATA, and pays no stream credit for it. It sends no keys, mouse reports, RESIZE,
FOCUS or FACTS. Normal attaches are unchanged; on a core without the feature,
the `size` event updates the terminal record, while the pane VT follows the
client layout and its own RESIZE.

**Reconnect**

- A dropped connection does **not** mean the programs died.
- Reconnect, `hello`, resubscribe; on `missed`, resnapshot; re-read documents; reattach terminals.
- On `lost`, the kept terminal stays live; wait for `restarted` and attach again.
- On `finished`, remove the stream but keep the record/scrollback as configured.

**Shutdown**

- Detach streams (CLOSE), restore the terminal, flush the document with the
  same bounded conflict/merge budget as a normal write, and save the local
  view; do not stop core unless the user explicitly asks.

### 4.8 Scrollback, search, palette, settings and themes

**Scrollback** is Ghostty's own bounded history per pane. Attach asks core for
history above the screen, so output printed before a screen attached is
scrollable. The line limit comes from config (default 10,000, maximum
1,000,000), paired with a byte budget capped at 64 MiB. Ghostty prunes complete
pages, so the retained rows can exceed the line limit by about one page, but
memory stays bounded. Wheel (when not forwarded), Shift-PgUp/PgDn (half a page),
Shift-Home (top) and Shift-End (live bottom) move the viewport; any ordinary key
returns to the live bottom before it is forwarded. The focused cursor is drawn
only at the live bottom. Reattach builds a fresh VT from core's paint, so
history is never duplicated. Scrollback is not written to the document or the
per-screen view.

**Search** (`prefix f`) runs a literal, ASCII-case-insensitive substring search
(no regex) over the focused pane's local history. It shows a match count, Enter or Down
moves to the next match, Up to the previous, and Esc closes. Search runs in
bounded steps from the live loop, so a large history cannot block input. It
never calls `terminal.read` and never affects the program.

**Palette** (`prefix :`) is a modal view over the existing command table,
filtered by a case-insensitive substring of the label and action name. Up/Down
move the selection, Enter runs it, Esc closes, and other typed characters —
including `j` and `k` — edit the filter. Each row shows the command and its
current configured key hint. A view-only command writes no document; a layout
command behaves exactly as its key does.

**Settings** are read once at startup from
`$XDG_CONFIG_HOME/herdr-terminal/config.json`, falling back to
`$HOME/.config/herdr-terminal/config.json` when the variable is unset or empty.
The fields are `theme` (`default`, `dark`, `light`), `scrollback` lines,
`mouse` on/off, `clipboard` (`osc52`, `native`, `both`, `off`) and `keys`
prefix-binding overrides. A missing file is normal. Malformed JSON, unknown
settings and bad values warn and fall back to defaults, so the TUI still starts.
Key overrides reject unknown actions, control keys, multi-character keys and
collisions, keeping the default binding. There is no settings UI and no live
reload.

**Themes** style only the chrome: tab bar, pane borders, status bar, sidebar,
peek, help, palette, search and local selection. Terminal cells always keep the
program's colors. `default` uses the outer terminal's palette; `dark` and
`light` use explicit colors mapped down for 16- and 256-color terminals, chosen
from `COLORTERM` and `TERM`.

**Performance.** Input handling, command dispatch, scrollback and search are
bounded per loop iteration, and output is drained in bounded batches with the
existing credit flow. A large output burst therefore does not block palette,
search or redraw input.

## 5. Feature scope

Statuses:

- **v1** — required for the first usable release.
- **Later** — planned; not yet scheduled.
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

| Feature | Notes |
|---|---|
| Tab reorder | Mouse and keyboard |
| Machines: add/connect/list/rename/rm | Remote terminals and events |
| Files: read/write/edit, diff, external editor | Core `file.*`; no embedded editor |
| File picker | Simple list; use `machine.exec`/`file.*` |
| Custom layouts, accessibility | Settings and config |
| Packaging, install, `--skill`, CLI docs | Distribution |
| Mods/plugins (manifest + external commands) | See §6.3 |
| Kitty graphics/images | See §6.3 |

### 5.3 Postponed

| Feature | Why postponed | Trigger |
|---|---|---|
| Mods/plugins | Needs a plugin model and a host API; not required for v1 | Later; use Herdr 1.x's manifest + external-command model, not JS/GPUI |
| Kitty graphics / images | Terminal support varies; VT binding work | Only if users need it |
| Embedded code editor | Large scope; cell-grid editor is a product of its own | Use external `$EDITOR`; revisit only if demanded |
| Markdown rendering | Rich text is out of scope for a TUI | Use an external pager/`glow`/`bat` |
| Advanced settings UI | Basic config first | Later; simple forms |
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

### 5.5 Implemented

§5.1–§5.4 describe the target scope; this subsection records what the code
currently implements.

- **Connection:** core discovery, one long-lived framed connection, the
  `hello` feature check, event subscription, and reconnect that keeps the
  programs running.
- **Terminals:** local VT rendering, keyboard and mouse input, resize, focus,
  display facts, credit-based flow control, and the attach lifecycle (`lost`,
  `restarted`, `finished`, reattach).
- **Workspace:** spaces, tabs, panes, split/close/move, focus and zoom.
- **Persistence:** the `herdr-terminal` document, restore, multi-screen
  compare-and-set, and terminal rediscovery by labels.
- **Agents:** status, turn, title and foreground from core; the agent
  sidebar; peek; jump; seen and pinned labels; blocked and finished alerts.
- **UI and CLI:** tab bar, pane frames, status bar, help overlay and rename
  prompt, with the `Ctrl-b` prefix key map and `?` for help; `ht` opens the
  workspace, `ht screen` runs or reattaches to one terminal and `ht probe`
  copies the raw attach bytes. `Ctrl-]` detaches and the programs keep
  running.
- **Cell width and startup size:** the local VT enables grapheme clustering
  (DEC mode 2027) by default like Core and Herdr App, so variation-selector
  emoji form one cluster two cells wide, followed by a zero-width spacer cell,
  and RIS keeps it on. Every spawn path sends the optional `terminal.run` size
  when the attach grid is known; older cores ignore the field and still work.
- **Observer attach (foundation):** the library can attach a read-only
  observer when the core advertises and confirms resize notifications; it
  applies RESIZE before the following DATA, pays no credit for it, and sends no
  input or control frames. No peek UI uses it yet; normal attaches are
  unchanged.
- **Mouse and history:** clicks focus panes and select tabs, split gaps drag,
  and chrome never reaches a program. Programs that track the mouse receive
  pane-local events; otherwise the wheel scrolls local history, requested
  from core at attach and bounded by the configured line and byte limits.
  Shift-PgUp/PgDn, Shift-Home/End and any ordinary key return to live.
- **Search and clipboard:** literal case-insensitive scrollback search with a
  match count and next/previous jumps, driven in bounded steps; local
  selection with no cell padding; copy through OSC 52 by default, optionally
  a native helper; bracketed paste only to a focused program that enabled
  DEC 2004.
- **Palette and settings:** the command palette reuses the command table and
  shows configured key hints; startup settings in
  `$XDG_CONFIG_HOME/herdr-terminal/config.json` cover theme, scrollback,
  mouse, clipboard and prefix key overrides. The default, dark and light
  themes style chrome only, mapped to 16, 256 and truecolor; terminal cells
  keep their own colors.
- **Performance:** bounded per-loop work, bounded output batches and bounded
  scrollback/search keep palette, search and redraw responsive under large
  output.

The implemented set covers the connection and terminal foundation, the
workspace and its persistence, agent awareness, the daily-driver input layer
(mouse, history, search, selection, clipboard, palette, settings and themes),
the local VT's grapheme clustering, initial terminal sizes and the
observer-attach foundation. Still planned: tab reorder, custom layouts,
accessibility, general confirm/toast prompts, machines, files,
packaging/install, `ht status` and mods. Scrollback, search,
selection and theme stay in memory and are never written to the shared
document or the per-screen view. Settings are read at startup; there is no
settings UI and no live reload.

The implementation is unreleased: there is no release, packaging or install
procedure yet. See §3 for the boundary rules, §7 for conventions, and
[AGENTS.md](../AGENTS.md) for the traps and the source layout.

## 6. Planned work

Current status is §5.5. The remaining work, grouped by capability:

### Workspace and UI

- Tab reorder.
- Custom layouts.
- Accessibility.
- General confirm/toast prompts.
- Performance tuning for many panes.

### Machines and files

- Machine add/connect/list/rename/rm.
- Remote terminals, events and documents.
- Reconnect after network loss.
- File read/write/edit; diff; external editor; simple file picker.

### Distribution and testing

- Packaging and install.
- `--skill`, CLI docs and CI.
- `ht status` and core/log diagnostics.
- End-to-end tests and golden buffers.

### Later

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

1. **VT binding:** resolved for v1: a TUI-local binding over the vendored VT
   library, because the TUI needs styled cells and key/mouse encoding that
   core's minimal binding does not expose. Extracting a shared crate or a
   pure-Rust VT remains a future option.
2. **Async runtime:** threads + channels vs tokio. Start simple; measure.
3. **Document layout:** one `herdr-terminal` document per core with machine-qualified ids, or one document per machine like Herdr App? Start local, revisit when remote machines are added.
4. **Clipboard:** resolved for v1: OSC 52 by default, with a configured native
   helper, both, or off; a native failure falls back to OSC 52.
5. **Notifications:** resolved for v1: one in-app status-bar notice and one
   BEL, with no OS notification. OSC 9/777 and platform notifications remain
   future options.
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
