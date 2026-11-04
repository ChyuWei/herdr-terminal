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
- **Shared layout, local UI.** The `herdr-terminal` document holds layout; focus, zoom and the rail's focus flag stay in the per-screen view; scrollback, search, selection, mode state, theme and terminal-/machine-manager state stay in memory, and open file contents stay out of the document and labels (only the editor's temp copy is local).
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
| `workspace` | Workspace model, split tree and geometry (with the 90-column boundary and narrow tests), commands, keybindings, palette, agents, rail, pane-nav, terminal manager, machine manager, file viewer, startup, sync, ratatui UI; `runtime/` holds the event loop, attach, input, mouse, overlays, resize mode, reconnect, manager, machine, file and agent handling; `ui/sidebar.rs` draws the rail and drawer, `ui/overlays.rs` draws overlays, prompts and the tab selector, and `ui/machine_ui.rs` draws the machine panel; `ui/tests.rs` keeps the shared goldens and delegates panel cases to `ui/tests/panels.rs` |
| `workspace::sidebar`, `workspace::panenav` | Pure rail width (hidden below 90 columns; a quarter of the viewport clamped to 20..30 including the separator), variable-height lines, scrolling, drawer rectangle and hit testing; pure narrow pane-nav chips with whole-chip scrolling and hit testing |
| `workspace::runtime::resize` | Keyboard resize mode that targets a split and calls the weighted `resize_gap` |
| `workspace::file`, `workspace::runtime::file` | Pure file context, 400-line paging, line diff, editor selection/temp rules and write-back rules; runtime file IO through Core and the editor session |
| `workspace::state`, `workspace::local` | Runtime UI state and per-screen view |
| `config` | Startup settings: theme names and aliases, scrollback, mouse, clipboard, prefix keys |
| `theme` | The default dark and light chrome roles, the earlier Catppuccin palettes under their names, the named-color `terminal` look, and 16/256/truecolor mapping |
| `clipboard` | OSC 52 encoding and native helper fallback |
| `tui`, `term`, `session`, `probe` | Terminal backend, process/session ownership, `ht screen` and `ht probe` |

The workspace model, geometry, commands, keys, palette, agents, rail and
pane-nav are pure and unit-testable without a PTY. IO lives in `core`, `screen`,
`vt`, `workspace/runtime`, `tui` and `main`. `ht` has no `status`, `commands` or
`--skill` subcommand yet; those remain planned.

### 4.3 Rendering pipeline

1. The attach stream delivers the **first paint** and then raw program bytes.
2. Feed bytes into the local VT; it owns screen, scrollback, cursor, modes, title and responses.
3. On redraw, update the VT render state and iterate visible rows/cells.
4. Map each cell's symbol, style, foreground/background and attributes into a ratatui `Buffer` cell; honor wide cells and continuation cells.
5. Compose the TUI around the terminal area: the rail on a wide terminal, the pane-nav on a narrow one, pane frames with their identity rows, one bottom line and the overlays.
6. Diff and present; use synchronized output when the terminal supports it; place the cursor only for the focused terminal.
7. Send **FACTS** when colors or cell size change, **RESIZE** when a pane changes size, **FOCUS** when focus changes.

The TUI owns the screen. It does not ask core for frames or scrape `terminal.read`.

**Space → Pane chrome.** A wide terminal puts the rail on the left; panes
begin at row 0 with no window bar, brand header, tab bar or per-pane footer.
Each pane carries one identity row in its top border: a focused marker, the
two-digit layout number, the name (with the title when it differs), and the
OSC 7501 word with the address on the right. The focused pane uses a mint
frame and header band; other panes use a hairline. In a tight header the
address drops first, then the title detail, so the number, name and word
survive. A narrow terminal hides the rail, adds one pane-nav row of chips
above the pane area, and draws only the focused pane, in the full workspace;
its chips keep split-tree numbers, read `NN name` with the same word when it
fits, and scroll as whole chips so the focused pane stays visible while every
pane remains reachable by click or keyboard. A zoomed tab draws only the
focused pane and hides the pane-nav. `prefix a` opens the rail as an opaque
drawer over the panes on a narrow terminal, ending above the bottom line; on a
wide terminal it only moves keyboard focus.

One bottom line spans the whole viewport: a mode chip (`NORMAL`, `PREFIX`,
`COPY`, `RESIZE`, or `ZOOM` while the active tab is zoomed and no other mode
is active), the focused identity (`address · app · cwd` when Core reported
them, otherwise the address and name/title), then context hints. The active
space appears only in the rail; the rest of the chrome never repeats it. Every
chrome color comes from `Theme`: page, panel and rail backgrounds, the bottom
bar, hairlines, muted/dim text, the mint focus color, amber blocked, blue
working and the active-row background. `default` and `dark` are the dark
palette; `light` is the light palette; `catppuccin` and `catppuccin-latte`
keep the earlier palettes, and `terminal` keeps the earlier look in the outer
terminal's named colors. Explicit colors are mapped to the nearest 256-color
index or the nearest of the 16 named colors from `COLORTERM` and `TERM`, so no
RGB reaches a terminal that cannot show it. Terminal cells always keep the
program's own colors.

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
2. An open overlay (help, prompt, palette, search, manager, machine manager
   with its add/rename prompt and removal confirmation, file overlay with its
   path prompt, viewer, diff and conflict states, tab selector, preview, peek)
   consumes every mouse event. A selection is not in that list: a fresh press
   leaves selection and is routed as if none existed.
3. On a narrow terminal an open rail drawer is modal: every mouse event is
   consumed, a press on a row selects it (and may switch a space or jump), an
   outside press dismisses the drawer, and the rest of that gesture is
   swallowed so it never reaches the pane underneath. On a wide terminal the
   rail is not modal: a click in it selects a space or jumps to an agent, and
   a click outside only returns its keyboard focus and is routed normally.
4. Chrome: a pane-nav chip focuses its pane, a bottom-line click is consumed,
   and a split-gap press starts a resize only on a wide, unzoomed tab. Chrome
   never reaches a program.
5. A pane-grid press focuses the pane; a border press only focuses it. With
   Shift, or when the program does not track the mouse, the press arms a local
   selection whose release stays local even without a drag. Selection starts
   only when the pointer reaches another cell, so press-and-release in one cell
   is a plain focus click (with Shift, a local click that sends nothing).
6. Without Shift, a tracking program receives press, drag and release in
   pane-local coordinates. Shift-drag selects locally instead.
7. The wheel is forwarded only to a tracking program with no active selection;
   otherwise it scrolls that pane's local history. A wheel over chrome is
   consumed.

RESIZE is sent only when a visible pane's grid actually changes. Overlays and
chrome never change pane geometry. A narrow view sends no RESIZE to a hidden
pane: it keeps running at its last grid size, new panes start at the visible
grid, and widening recomputes every pane.

**Keyboard modes and the bottom line**

PREFIX (the `Ctrl-b` prefix chord), COPY (`prefix [` selection) and RESIZE
(`prefix r`) each own the keys until they end; ZOOM is the persistent state of
a zoomed tab, not a key-owning mode. The one bottom line names the active mode
in its chip and shows that mode's keys on the right; there is no separate mode
strip. The chip prefers resize, then copy, then prefix, then `ZOOM` for a
zoomed tab. Opening an overlay or search, giving the rail focus, or
reconnecting calls `clear_modes`, and a mouse-started local selection leaves
resize first, so the chip and the keys that act always agree.
Other keys leave COPY and reach the program; RESIZE swallows a key that is not
a direction and leaves without acting. A mode never resizes a terminal.

**Resize mode** (`prefix r`) needs a wide (90-column), unzoomed tab with more
than one pane; a narrow or zoomed tab reports why resize is unavailable, and
an empty tab and a single-pane tab get distinct messages. `h`/`l` move the
focused split on the column axis and `j`/`k` on the row axis by one cell,
Shift by five; Esc or `q` leaves, and a missing split on that axis is a no-op
that stays in the mode. It picks the boundary with `geometry::resize_target`
and calls the same weighted `resize_gap` as a mouse drag, so the keyboard and
the pointer converge on one layout; pane minimums hold, and the new grid is
sent as a normal RESIZE.

**Tabs without a tab bar.** The tab model, its document schema and restore are
unchanged, but the screen draws no tab bar or fixed tab categories. `prefix
n`/`p` cycle tabs, `prefix 1..9` selects one directly, and `prefix T` opens a
modal selector for the active space listing each tab's index, name, pane count
and an active marker. `j`/`k` or the arrows move, `g`/`G` jump to the first or
last tab, Enter switches and closes, and Esc or `q` closes without switching.
A rail space row's tab count is informational: clicking the row switches to
the space's own active tab and does not open the selector.

**Selection, copy and paste**

- `prefix [` starts a keyboard selection at the focused cursor; h/j/k/l or the
  arrows extend it by a cell, `w`/`b`/`e` move by word, `v` or space re-anchors
  the selection at its end, `y` or Enter copies, `/` or `?` opens the same
  scrollback search and ends the selection, and Esc cancels. Mouse click-drag
  selects on a non-tracking grid; Shift-drag selects over a tracking program.
  Selection is cell-based; word- and line-granular selection modes are not
  provided.
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
history and whether the rail holds keyboard focus. The screen id comes from
`HERDR_TERMINAL_SCREEN` (1–64 characters of
`[A-Za-z0-9._-]`, default `default`). A missing or malformed view is a warning,
not an error; ids that no longer exist are ignored. Scrollback, search,
selection, mode state, the rail selection/offset and theme are runtime-only:
they are not stored here or in the shared document.

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
tab), not in place of a restored pane. Restored and recovered panes that carry
this client's label and match their own pane label are workspace-owned for
close semantics (`x` ends a non-kept program, `e` detaches, `X` kills), while
their `Session` is adopted so a local startup or fatal error never kills a
program this process did not start. A manager-shown pane is matched by the
document's address and restored unowned: it is a viewer for that program, so
`x`/`e` detach and `X` kills. A missing or stale pane label never grants
ownership.

### 4.6 Agents, attention and the rail

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

**Rail** (`prefix a`) is the left panel of the Space → Pane screen: a
persistent column on a wide terminal and an on-demand drawer on a narrow one.
On viewports at least 90 columns wide it takes the left edge, a quarter of the
width clamped to 20..30 columns including the separator (22 at 90 columns, 30
at 120); panes start to its right at row 0, and the bottom line spans the full
viewport width. It shows `SPACES` with a right-aligned count, then each space
as a row with its two-digit index, name and tab count; a space adds a dim
folder line only when its model has a non-empty folder. A dim rule separates
the sections when a spare row allows it. `AGENTS` follows with its own count
and an optional dim attention summary (`N blocked · N done unread`), then
every agent across all spaces and tabs on two lines: a pin or unread marker,
the title and the Core word on the right, then `space · address`. Agents sort
pinned first, then blocked > done > working > idle, then workspace order and
address; unknown words sort with idle. Plain shells are never listed, and a
word exists only because OSC 7501 reported one. The pane identity row and a
narrow chip share `panenav::word`: an agent row turns a reported `error` into
`blocked` and a seen `done` into `idle`, and without a row the pane's raw OSC
word shows as-is. When the rail is tall enough, a pinned footer reads `^b a
navigate · Enter jump`.

`j`/`k` or the arrows move, `g`/`G` go to the first/last item, `Enter`
switches to a space or jumps to an agent (a jump returns keyboard focus to the
pane), `i` peeks, `p` toggles the pin, `u` marks unread, and `Esc` or `a`
returns focus. Either line of a space or agent row selects it; clicking a
space switches to the space and its own active tab, while clicking an agent
jumps. The tab count is informational and does not open a selector.

A focused rail counts as covering the panes for seen/attention, but on a wide
terminal it is not mouse-modal: a click outside returns keyboard focus and is
routed normally. Below 90 columns the rail is hidden and `prefix a` opens the
same model as an opaque drawer over the panes, ending above the bottom line.
The drawer never resizes a pane, changes the split tree or writes the
document; while it is open every mouse event is consumed, an outside press
only dismisses it, and the rest of that gesture is swallowed. Losing the panel
releases its focus, and widening the terminal does not restore it. Selection
is by item (space id or pane id) and follows reordering. The focus flag is
local view state (`sidebar_open`, schema v1); the selection and offset are not
persisted.

**Peek** (`prefix i`, or `i` on a rail row) draws the pane's existing local
VT plus core facts (title, address, word, turn, pin, app, progress, kind,
message). It never calls `terminal.read` and never infers status from the
text. Any key closes it; `Ctrl-]` still detaches.

**Jump** (`prefix g`/`G`) cycles agents in rail order, clears zoom, and does
not write the document.

**Attention**: after a baseline built from the first snapshot, a newly
`blocked` row and a hidden busy→non-busy transition produce one in-app
bottom-line notice and one BEL; no OS notification. Looking at a terminal
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

### 4.7 Terminal manager and preview

**Manager** (`prefix t`; palette "terminal manager", action `terminals`) is a
modal overlay. It lists every live terminal in the connected Core from the
existing `events.subscribe` model, including terminals that are not in this
workspace; it never calls `terminal.read` and never reads screen text. A row
shows `*` when the terminal is shown here, its OSC 7501 word (`unknown` when
the program never reported), age and address, then name, title, parent,
machine and cwd when present. Rows sort blocked/error, working, done, idle,
unknown, then oldest first and address. `j`/`k` or the arrows move, `g`/`G` go
to the first/last row, `/` edits the filter, and Esc closes; the filter is a
case-insensitive substring of the facts the row shows. At narrow widths a row
keeps the marker, status word and address, then adds name and the other facts
only while they fit; it shortens the address with `…` rather than wrapping.
Selection, filter and scroll offset live in `UiState`; they are not persisted
in the shared document or the per-screen view.

`Enter` toggles the selected terminal. Showing a hidden terminal inserts a pane
that adopts the existing address (`owned = false`) and attaches to the program
already running; it never calls `terminal.run` and never spawns or respawns. A
terminal already shown is focused instead of adding a second pane. Hiding
detaches the pane's stream and removes the pane from the layout; it never sends
`terminal.kill` or `terminal.rm`, so the program keeps running and can be shown
again by address.

**Preview** (`p`) attaches the selected terminal as a read-only observer,
renders it through a local VT, applies each RESIZE before the DATA that follows
it, and sends no keys, mouse reports, RESIZE, FOCUS or FACTS. It does not
change the workspace or the document. Any key or Esc closes it, and no key
reaches the manager or the program. The local-VT agent peek is a separate
feature and is unchanged.

**Hide labels and ownership.** Hide always removes the pane from the shared
layout. It clears the four identity labels (`herdr.client`,
`herdr.terminal.space`, `herdr.terminal.tab`, `herdr.terminal.pane`) only when
the terminal carries this client's `herdr.client` value and its pane label is
in this screen's id namespace. Labels written by another `ht` screen are left
for that screen's recovery, so Hide is durable across restarts only for
terminals this screen created; a later label-based recovery may re-add a
terminal whose labels were not cleared. Durable cross-screen hide would need a
shared-document marker, which this feature does not have. A manager-shown pane
is captured in the document and restored by address as unowned: it is a
viewer, so `x`/`e` detach and `X` kills.

### 4.8 Machine manager and remote terminals

**Manager** (`prefix m`; palette "machine manager", action `machines`) is a
modal overlay over `machine.list`. It is separate from the terminal manager:
`t` lists terminals, while `m` lists the machines Core can reach. Opening it
closes the other overlays, and opening the terminal manager closes it. Core
reports machine state but emits no machine-state event, so the list is
refreshed only when the overlay opens, after each action, and on a short
bounded timer while it stays open; a closed manager never calls
`machine.list`. A `missed` snapshot while it is open refreshes it at the next
loop step. Selection, filter and scroll offset live in `UiState`; they are
not written to the document or the per-screen view.

A row shows the name Core uses in `machine/t7` addresses, its state
(`connecting`, `connected`, `needs_login`, `needs_update`, `unreachable`, or
`unknown` when the reply carries no state), the elapsed time of its current
state and Core's reason when present, its target, host and version, its
session, and the `temporary` and `via` facts when present. Rows sort connected,
connecting, `needs_login`, `needs_update`, `unreachable`, then unknown or
future states; permanent machines precede temporary ones in the same group,
then case-insensitive name. At narrow widths a row keeps name, state and
target and adds the secondary facts only while they fit. `/` filters with a
case-insensitive substring over the facts the row shows. `j`/`k` or the
arrows move, `g`/`G` go to the first/last row, and Esc closes. While the
overlay or its prompt/confirmation is open, every key is consumed and none
reaches a program.

**Actions.** `Enter` calls `machine.connect` for the selected machine, which
connects or retries; Core owns the retry loop and the client reports only the
state it returns. `a` opens a one-line target prompt: the text is passed to
`machine.add` unchanged, including `user@host:port`; the client does not
parse, rewrite or add a `via` path, and Core chooses the name. `r` opens a
rename prompt prefilled with the current name. `d` asks for confirmation and
then calls `machine.rm`; Core aborts the line and its terminals leave through
Core's `removed` events, never through a local guess. `n` starts a new
terminal on the selected machine and closes the manager so the pane is
visible. Errors and Core's refusals stay inside the overlay. A core without
`machine.list` gets an upgrade message there, and the client never calls the
missing method.

**Remote terminals.** A start from the manager uses this client's one
connection and its normal labels, with `terminal.run` `on` naming the machine;
the reply's `machine/t7` address is stored unchanged and the terminal behaves
as an ordinary workspace pane: input, resize, focus, FACTS, terminal-manager
Show/Hide and observer Preview all apply. Pane titles and the bottom line show
the machine prefix. Remote panes are captured in the local `herdr-terminal`
document with their full qualified address and restored by it. A short `t7`
is never assumed unique across machines: identity is the whole address, so a
local `t7` and `alpha/t7` are different panes.

**Rename and recovery.** Core owns the machine line (SSH or a `via` command),
dialing, backoff, mirror replacement and relayed streams; `ht` never dials,
adds an SSH path, or runs a private remote reconnect loop. A machine rename
reaches the client as `removed` for the old qualified addresses followed by
`created` for the new ones, or in a `missed` snapshot. The client re-addresses
its existing pane by the `herdr.terminal.pane` label, keeps the pane, its
attach stream and any final screen, and writes the new qualified address. On
`missed` it marks the document stale, fetches and merges it, and only then
reconciles the snapshot, as the reconnect path does. If the new address is
already shown, the move is refused rather than creating a second pane for one
terminal; the old pane leaves once Core no longer has its old address, and
the pane holding the address keeps it. A stream-less `removed` waits one loop
step so a following `created` can move the pane first. A terminal Core truly
removed still leaves through its `removed` event; no liveness is inferred
from screen text or process names, and no recovery path calls `terminal.run`
or spawns a replacement.

**Local document.** The workspace, including remote panes, stays in the local
`herdr-terminal` document. This feature reads and writes no remote document
(any later remote document use must call `doc.get`/`doc.set` with `on`), and
never touches Herdr App's document or `herdr.space`/`herdr.tab` labels.

### 4.9 Files: viewer, editor and write-back

**Overlay and context.** `prefix o` (palette "file viewer", action `files`)
opens a modal file overlay and closes the other overlays. While it or its
prompt, viewer, diff or conflict state is open, every key is consumed and none
reaches a program. The context comes from the focused pane when the overlay
opens: a qualified `machine/t7` address sends `on` and the terminal's cwd; a
local pane omits `on` but still sends its cwd; with no pane both are omitted,
so Core's default applies. The path prompt starts at that cwd with a trailing
separator, or empty. There is no directory listing or file picker: the path
prompt is the picker, and a trailing `/` or `\` is refused before any read;
other directory paths surface Core's IO error. Esc closes the prompt.

**Viewer.** The viewer is read-only and paged. It calls `file.read` with
`offset` and `limit` for a 400-line window and never loads the whole file for
display. A header names the machine (`local` or its name), the path, the
loaded range and the file's total line count (`lines 1-400 of 900`,
`line 3 of 5`, or `empty file`). `j`/`k` or the arrows move a line, PgUp/PgDn
move a screen, and `g`/`G` or Home/End go to the first/last line; leaving the
loaded window fetches the next or previous 400-line page. Esc closes. Read
errors stay in the overlay, under a heading that names the machine and path,
and leave the TUI usable.

**Editor and diff.** `e` makes one unbounded `file.read` and requires a
complete reply before it looks up an editor or creates a temp. It chooses
`$VISUAL`, else `$EDITOR`; the value is split on whitespace (no shell
quoting) and the temp path is appended, with no `vi` fallback. The client
writes a unique local temp file `ht-<pid>-<serial>-<basename>` with
create-new, mode 0600 where supported, and the source basename and extension
preserved, then suspends the TUI: it leaves the alternate screen and raw mode
and restores keyboard enhancement, paste, focus and mouse on return, even when
the editor fails or exits nonzero. The temp is read back and removed. An
unchanged buffer returns to the viewer; a changed one becomes a line diff
(LCS, `+`/`-` marks, an added/removed summary; a large alignment is shown as
one changed block and the row list is capped). The diff is a display aid only:
there is no merge or three-way resolution.

**Write-back.** `w`/Enter re-reads the whole file and compares it with the
first full read; Core has no file version or etag. The re-read and the write
are separate Core requests, so the check is best-effort, not a transaction: a
change made between them is not detected. The shared workspace document is
different: `doc.get` returns a version and `doc.set` takes the expected
version, so its updates are optimistic compare-and-set. A detected difference
shows a conflict and refuses to write; `o` in that state is the explicit
overwrite. `q`/Esc discards without writing. Without a conflict, one exact
contiguous replacement uses `file.edit`; every other change, and the explicit
overwrite, uses `file.write` with base64 data. A failed write keeps the diff
and buffer with the error. A whole read that Core truncates at its inline cap
(about 8 MB of estimated JSON bytes; `last < lines`) is refused before the
editor starts and again on the write re-read, before any comparison or write,
so a partial buffer is never edited, compared or saved. The viewer can still
page through such a file. The refusal names the machine, path and read/known
line counts.

**Boundary.** Core reads and writes the file, local or remote, through this
client's one long-lived connection; the client never opens a remote path and
does no direct path IO for the file itself. Contents stay in the in-memory
file session: they never enter the `herdr-terminal` document, labels,
another client's document or a Core document. Only the editor's temp copy is
local.
`file.read`, `file.write` and `file.edit` are required at connect; the minimum
Core stays 0.1.17, and the existing `hello` check refuses a core without them,
with no fallback. There is no directory listing, embedded editor, merge,
three-way resolution or direct local/remote path IO.

### 4.10 Core client and lifecycle

**Supported Core**

Core 0.1.17 or newer at protocol generation 1, with the required methods and
features (including `resize_notifications`). Older or unreadable versions are
refused at connect with a message naming the reported and minimum versions and
the update path. There is no reduced-capability mode. Other optional
capabilities are gated on `hello.features`, never a version string.

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

1. Discover the core binary (configured path, `PATH`, login shell `PATH`, usual install dirs; accept it only if `--version` prints `hc …`). The probe is deadline-bounded and briefly retries a candidate whose executable is busy (for example while an installer replaces it) before rejecting it.
2. Connect to the socket or `hc connect`; send `hello {"framed": true}`.
3. Check generation, minimum version, methods and features; fail with a clear message if a required method or feature is missing, or the core is older than 0.1.17 or its version is unreadable.
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

**Start directory**

A new local terminal starts in the focused pane's Core-reported cwd when Core
has one, otherwise the client process's cwd, and `$HOME` when that cwd is `/`
or unreadable. The first shell has no pane, so it uses the client rule; an
explicit PROGRAM started into a restored workspace follows the focused pane
too. A start with `on` sends no cwd, so the machine's home applies; a local
path is never sent to a machine. A replacement shell starts in the exited
terminal's recorded cwd when it has one (a path on that machine for
`machine/t7`), then the client rule for a local pane.

**Initial size**

Every spawn path sends `terminal.run`'s optional `size` when it knows the grid
the terminal will attach at: the initial shell, a new pane/split/tab/space, an
explicit PROGRAM after restore, `ht screen`, a replacement after reconnect or
an exit, and `ht probe`. Workspace starts use the pending pane's exact inner
grid, previewed on a clone of the deterministic insertion so a failed run
leaves no empty tab.
A path that cannot know the size omits the field and core's default applies.
The minimum supported Core applies the field, so no version gate is used. This
matters because a program that reads its size once at startup, before a viewer
attaches, would otherwise draw its first frame at core's default size.

**Observer attach**

The terminal manager's Preview attaches a terminal as a read-only observer; the
local-VT agent peek is a different feature and does not use this path.
`terminal.attach` takes `observe` and `resize_notifications`; the connection
requires the feature, so the client requests both unconditionally and consults
no Core version string. The reply must confirm the feature, or the client
abandons the channel and fails instead of falling back to a normal attach that
would take over the grid. A confirmed observer receives RESIZE (kind 3, four
little-endian bytes) before each paint that uses a new grid, applies it to the
local VT before the following DATA, and pays no stream credit for it. It sends
no keys, mouse reports, RESIZE, FOCUS or FACTS. Normal attaches are unchanged.

**Reconnect**

- A dropped connection does **not** mean the programs died.
- Reconnect, `hello`, resubscribe; on `missed`, resnapshot; re-read documents; reattach terminals.
- On `lost`, the kept terminal stays live; wait for `restarted` and attach again.
- On `finished` for a program in a terminal this screen started, close the
  pane: stop the stream, remove the finished record with `terminal.rm` (best
  effort), and remove the pane. If it was the tab's last pane, start one fresh
  shell in the same space and tab (the same machine for `machine/t7`); a
  background tab's replacement does not switch the active space or tab, and
  one that exits almost immediately is not replaced again.
- A restored or adopted record that was already finished keeps its final
  screen until closed; reconnects and `missed` snapshots preserve that
  behavior.

**Shutdown**

- Detach streams (CLOSE), restore the terminal, flush the document with the
  same bounded conflict/merge budget as a normal write, and save the local
  view; do not stop core unless the user explicitly asks.

### 4.11 Scrollback, search, palette, settings and themes

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
The fields are `theme` (`default`, `dark`, `light`, `terminal`, plus the
`catppuccin` and `catppuccin-latte` aliases), `scrollback` lines,
`mouse` on/off, `clipboard` (`osc52`, `native`, `both`, `off`) and `keys`
prefix-binding overrides. `default` and `dark` select the dark chrome,
`light` the light chrome, `catppuccin`/`catppuccin-latte` the earlier
Catppuccin palettes, and `terminal` the named-color look. A missing file is
normal. Malformed JSON, unknown settings and bad values warn and fall back to
defaults, so the TUI still starts. Key overrides reject unknown actions,
control keys, multi-character keys and collisions, keeping the default
binding. There is no settings UI and no live reload.

**Themes** style only the chrome: the rail, pane frames and identity rows, the
bottom line, overlays, the tab selector, peek, help, palette, search and local
selection. Terminal cells always keep the program's colors. `default` and
`dark` select the dark palette and `light` the light palette. `catppuccin`
and `catppuccin-latte` keep the earlier Catppuccin palettes, and `terminal`
keeps the earlier look in the outer terminal's own named palette, reversing
selected rows because it has no surface color. Every role — page, panel, rail
side, bottom bar, hairlines, muted and primary text, mint focus, amber
blocked, blue working and the active-row background — exists in the explicit
palettes; explicit colors are mapped to the nearest 256-color index or the
nearest of the 16 named colors from `COLORTERM` and `TERM`, so no RGB reaches
a lesser terminal. Agent words use only the Core-reported status word:
working blue, blocked amber, a raw error red, done mint, idle dim, anything
else muted; the client's `exited N` fact is red.

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
| Terminal manager | Core-wide live-terminal inventory, Show/Hide, read-only Preview |
| Machine manager | `machine.list` machines, connect/add/rename/remove, remote `machine/t7` terminals |
| Files | Paged read-only viewer, `$VISUAL`/`$EDITOR`, line diff, content-checked (non-atomic) write-back |
| Terminals | New/close/rename, labels, keep, kill/rm, shell/command |
| Agents | Status, turn, title, foreground, spaces/agents rail, narrow pane-nav, peek, seen/pinned |
| UI | Space → Pane chrome: rail and drawer, pane identity rows, narrow pane-nav, one bottom line, tab selector, command palette, help, toast/confirm/prompt |
| Persistence | Own `herdr-terminal` document, restore, multi-screen CAS |
| Themes | Default dark/light chrome, Catppuccin aliases, named-color `terminal` theme, 16/256/truecolor mapping |
| Recovery | `lost`/`restarted`/`finished`, `missed`, heartbeat, reattach |
| Diagnostics | Version, core status, logs, `ht status` |
| Distribution | macOS Apple Silicon package with notices, GitHub Actions CI and a tag-triggered GitHub release workflow |

### 5.2 Expected later

| Feature | Notes |
|---|---|
| Tab reorder | Mouse and keyboard |
| File picker / directory listing | Simple list; the path prompt is the picker today |
| Custom layouts, accessibility | Settings and config |
| Install, `--skill`, CLI docs | Distribution |
| Non-macOS packaging | When those platforms become release targets |
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
  display facts, credit-based flow control, start directories from the focused
  pane, client cwd or home, and the attach lifecycle (`lost`, `restarted`,
  `finished`, reattach). A terminal this screen started closes its pane on exit
  and replaces a tab's last pane with one fresh shell on the same machine; a
  restored or adopted finished record keeps its final screen until closed.
- **Workspace:** spaces, tabs, panes, split/close/move, focus and zoom.
- **Terminal manager:** a Core-wide modal inventory of every live terminal
  from the event model, with a shown marker, status word, age and optional
  facts; status sort and substring filter; Show adopts an existing address and
  Hide detaches without kill/rm; a read-only observer Preview; selection and
  filter live in `UiState`.
- **Machine manager:** a modal `machine.list` inventory with state, since/why
  and connection facts, sorted and filtered locally; connect, target-only add,
  rename and confirmed remove; `terminal.run` with `on` starts a remote
  terminal whose qualified `machine/t7` address persists in the local
  document and appears in pane headers and the bottom line; label-based
  re-addressing keeps
  panes, streams and final screens across a Core rename or `missed` snapshot,
  without duplicates or replacement spawns. The list refreshes only while the
  overlay is open; Core has no machine-state event.
- **Files:** a modal `prefix o` path prompt (no directory listing), a read-only
  400-line paged `file.read` viewer, `$VISUAL`/`$EDITOR` on a local temp with
  full TUI suspend/restore, a line diff, content-checked (non-atomic)
  write-back with an explicit `o` overwrite, and a refusal to edit or write a
  whole read truncated at Core's inline cap; contents stay out of the document
  and labels.
- **Persistence:** the `herdr-terminal` document, restore, multi-screen
  compare-and-set, and terminal rediscovery by labels.
- **Agents:** status, turn, title and foreground from core; the Space → Pane
  rail with spaces (index, name, tab count and optional folder line) and
  two-line cross-space agent rows, plus its narrow drawer; peek; jump; seen and
  pinned labels; blocked and finished alerts.
- **UI and CLI:** a Space → Pane screen with no window bar, brand header, tab
  bar or per-pane footer; panes from the top with one identity row in each top
  border (a narrow terminal adds one pane-nav row above them), a rail or
  pane-nav, one bottom line carrying the mode chip, focused identity and
  hints, a tab selector, help overlay and rename prompt, and default dark/light
  chrome with Catppuccin aliases and a named-color `terminal` theme; the
  `Ctrl-b` prefix key map and `?` for help; `ht` opens the workspace, `ht
  screen` runs or reattaches to one terminal and `ht probe` copies the raw
  attach bytes. `Ctrl-]` detaches and the programs keep running.
- **Cell width and startup size:** the local VT enables grapheme clustering
  (DEC mode 2027) by default like Core and Herdr App, so variation-selector
  emoji form one cluster two cells wide, followed by a zero-width spacer cell,
  and RIS keeps it on. Every spawn path sends the optional `terminal.run` size
  when the attach grid is known; the minimum supported Core applies it, so no
  version gate is needed.
- **Observer attach:** the connection requires `resize_notifications`, and the
  attach reply must confirm it; the terminal manager's Preview then attaches a
  read-only observer. It applies RESIZE before the following DATA, pays no
  credit for it, and sends no input or control frames. Normal attaches are
  unchanged.
- **Mouse and history:** clicks focus panes, rail rows and narrow pane-nav
  chips, a bottom-line click is consumed, split gaps drag only on a wide,
  unzoomed tab, and chrome never reaches a program. Programs that track the
  mouse receive pane-local events; otherwise the wheel scrolls local history,
  requested from core at attach and bounded by the configured line and byte
  limits. Shift-PgUp/PgDn, Shift-Home/End and any ordinary key return to live.
- **Search and clipboard:** literal case-insensitive scrollback search with a
  match count and next/previous jumps, driven in bounded steps; local
  selection with cell and word motion, re-anchoring and no cell padding; copy
  through OSC 52 by default, optionally a native helper; bracketed paste only
  to a focused program that enabled DEC 2004.
- **Palette and settings:** the command palette reuses the command table and
  shows configured key hints; startup settings in
  `$XDG_CONFIG_HOME/herdr-terminal/config.json` cover theme, scrollback,
  mouse, clipboard and prefix key overrides. `default` and `dark` select the
  dark chrome and `light` the light chrome; `catppuccin`/`catppuccin-latte`
  keep the earlier palettes and `terminal` the named-color look; every role
  maps to 16, 256 and truecolor; terminal cells keep their own colors.
- **Performance:** bounded per-loop work, bounded output batches and bounded
  scrollback/search keep palette, search and redraw responsive under large
  output.

The implemented set covers the connection and terminal foundation, the
workspace and its persistence, agent awareness, the daily-driver input layer
(mouse, history, search, selection and copy-mode motion, clipboard, palette,
the Space → Pane rail and its drawer, the narrow pane-nav, the single bottom
line and tab selector, keyboard resize, settings and dark/light themes), the
local VT's grapheme clustering, initial terminal sizes, start directories and
exit-driven pane replacement, the Core-wide terminal manager and its read-only
Preview, the machine manager with its remote terminals and rename recovery,
and the Core-backed file viewer, editor, line diff and write-back. Packaging,
notices, CI and versioning are also implemented; the package is macOS Apple
Silicon. Still planned: tab reorder, custom layouts, accessibility, general
confirm/toast prompts, the file picker and directory listing, install scripts,
`ht status` and mods. Scrollback, search, selection, theme, terminal- and
machine-manager state, and open file contents stay out of the shared document
and the per-screen view. Settings are read at startup; there is no settings UI
and no live reload.

The implementation is unreleased: the macOS package and CI exist, but no
version has been tagged yet. See §3 for the boundary rules, §7 for conventions,
and [AGENTS.md](../AGENTS.md) for the traps and the source layout.

## 6. Planned work

Current status is §5.5. The remaining work, grouped by capability:

### Workspace and UI

- Tab reorder.
- Custom layouts.
- Accessibility.
- General confirm/toast prompts.
- Performance tuning for many panes.

### File picker and future remote state

- File picker and directory listing; the path prompt is the picker today.
- Remote documents remain unimplemented: the workspace document is local.
  Any future remote document access must use `doc.get`/`doc.set` with `on`.

### Distribution and testing

- Install script and non-macOS packaging.
- `--skill` and CLI docs.
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
3. **Document layout:** one `herdr-terminal` document per core with machine-qualified ids, or one document per machine like Herdr App? The workspace document stays local and holds qualified addresses; revisit only if remote or shared layout state is designed.
4. **Clipboard:** resolved for v1: OSC 52 by default, with a configured native
   helper, both, or off; a native failure falls back to OSC 52.
5. **Notifications:** resolved for v1: one in-app bottom-line notice and one
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
