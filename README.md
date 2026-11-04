[English](README.md) | [简体中文](README.zh.md)

# HerdR Terminal

The terminal client for **Herdr Core**.

HerdR Terminal is a terminal UI (TUI) client of **Herdr Core**, the runtime that runs the programs and keeps their terminals alive. It is a sibling of the Herdr App desktop client and a replacement for the old Herdr 1.x TUI. HerdR Terminal owns the screen and workspace organization; Herdr Core owns the programs and the facts about them.

**Status:** implemented and unreleased. The client connects to Herdr Core, renders terminals through a local VT, runs a spaces/tabs/panes workspace with restore and agent awareness, and adds daily-driver input: mouse, bounded scrollback and search, selection with copy/paste, a command palette, and startup settings with default/dark/light themes, plus a Core-wide terminal manager, a machine manager for Core machines and remote terminals, and a Core-backed file viewer with an external editor, line diff and write-back. A file picker and the other later features remain planned.

**Compatibility:** Herdr Core 0.1.17 or newer, protocol generation 1. Older or unreadable versions are refused at connect with an update hint; a core missing a required method or feature is also refused. `resize_notifications` is required by the connection, not optional (observer attach has no fallback); other capabilities are gated on the `hello` features a core advertises, not a version string.

## Features

**Implemented**

- **Connection:** Core discovery, one long-lived framed connection, `hello` check, reconnect without losing running programs.
- **Terminals:** Run, attach, local VT rendering, keyboard and mouse input, resize, focus, display facts, credit-based flow control, and the `lost`/`restarted`/`finished` lifecycle. A new local terminal starts in the focused pane's Core-reported cwd, otherwise the client's cwd (or `$HOME` when that is `/` or unreadable); a start on a machine sends no local cwd and uses that machine's home (a replacement keeps the exited terminal's cwd), and an explicit `ht PROG` in a restored workspace follows the focused pane too. When a program in a terminal this screen started exits, its pane closes (stream detached, finished record removed with `terminal.rm` where possible) and, if it was the tab's last pane, one fresh shell starts in the same space and tab, on the same machine for a `machine/t7` pane, without switching a background tab into view; a shell that exits almost immediately is not replaced again, and a restored or adopted record that was already finished keeps its final screen until closed.
- **Text rendering:** Grapheme clustering is on by default, matching Core and Herdr App, so variation-selector emoji such as `✍️` and `❤️` form one cluster two cells wide, followed by a zero-width spacer cell, instead of splitting across cells.
- **Terminal sizing:** New terminals are spawned with the grid they will attach at — the initial shell, splits/tabs/spaces, explicit programs, `ht screen`, reconnect and exit replacements and `ht probe` — so a program that reads its size once starts with the right layout. Every supported Core applies it, so no version gate is needed.
- **Workspace:** Create spaces, tabs and panes; split/close/move; focus, zoom; rename tabs and spaces.
- **Terminal operations:** Labels, keep, kill/rm, shell or command.
- **Terminal manager:** `prefix t` opens a Core-wide list of every live terminal, with a marker for panes shown here, its OSC 7501 status (or `unknown`), age, address, and optional name/title/parent/machine/cwd; rows sort blocked/error first, then working, done, idle and unknown, and filter by case-insensitive substring. `Enter` shows a hidden terminal by adopting its existing address (no new program) or hides a shown one by detaching and removing the pane (never kill/rm); `p` previews it read-only through an observer attach, separate from the agent peek. Selection and filter are local UI state.
- **Machine manager:** `prefix m` opens a modal list of the machines Core knows, from `machine.list`, with name, state (connecting, connected, needs_login, needs_update, unreachable or unknown), the elapsed time of its current state and Core's reason when present, target, host/version, session, and temporary/via facts; rows sort connected, connecting, needs_login, needs_update, unreachable and unknown, with permanent machines ahead of temporary ones in the same state, and filter by case-insensitive substring. `Enter` connects or retries, `a` adds by target (passed to Core unchanged), `r` renames, `d` removes after confirmation, and `n` starts a terminal on the selected machine. A remote terminal gets a `machine/t7` address, lives in this workspace like a local pane, and restores from the local document; Core owns the machine line (SSH or `via`), dialing, backoff and relayed streams, while the client follows a machine rename by pane label and keeps the pane and stream. The list refreshes when the overlay opens, after each action, and on a timer only while it is open; Core has no machine-state event.
- **Files:** `prefix o` opens a modal file overlay. It reads and writes through Core, using the focused pane's machine (as `on`) and cwd when present; the path prompt starts at that cwd, and there is no directory listing or picker (a trailing separator is refused without a read). The viewer is read-only and pages 400 lines at a time with `file.read`, showing the machine, path, loaded range and total lines; j/k or arrows, PgUp/PgDn, g/G or Home/End scroll and fetch the next or previous page; Esc closes. `e` reads the whole file through Core and opens a unique local temp copy in `$VISUAL` or `$EDITOR` (split on whitespace, with no `vi` fallback), suspending and restoring the TUI; the result is shown as a `+`/`-` line diff. `w`/Enter writes and `q`/Esc discards. Before writing, the file is re-read and compared with the first read; a change shows a conflict and `o` is the explicit overwrite. The comparison is not atomic: Core has no file version, so a change made between the re-read and the write is not detected. Without a conflict, one exact contiguous change uses `file.edit`; otherwise `file.write`, including the explicit overwrite. A whole read that Core truncates at its inline cap is refused for editing and writing, while the viewer still pages through it. File contents never enter the document or labels; only the editor's temp copy is written locally. `file.read`, `file.write` and `file.edit` are required at connect.
- **Agents:** Status, turn, title, foreground, agent sidebar, local-VT peek, jump, seen/pinned, blocked/finished alerts.
- **State:** Own `herdr-terminal` document, restore, multi-screen sync; rediscover terminals by labels.
- **Recovery:** Detach keeps programs running; reconnect, reattach, and resnapshot after a missed event.
- **Mouse:** Click focuses a pane or selects a tab; dragging a split gap resizes; chrome never reaches a program. The wheel scrolls local history unless the program tracks the mouse; Shift-drag selects locally, even over a program that tracks the mouse.
- **Scrollback and search:** Bounded per-pane history requested from Core at attach; wheel and Shift-PgUp/PgDn/Home/End scroll, and any ordinary key returns to live. `prefix f` runs literal, case-insensitive substring search (no regex) with a match count and next/previous jumps. History is line-capped with a byte budget; the VT prunes by page, so retained rows can exceed the cap by about one page.
- **Selection and clipboard:** `prefix [` or drag to select; `y`/Enter copy; OSC 52 by default, with a configurable native helper (`pbcopy`, `wl-copy` or `xclip`) and bracketed paste. Selection is cell-based; word and line selection are not provided.
- **Command palette:** `prefix :` filters the existing command table case-insensitively and shows current key hints; Up/Down move the selection, Enter runs it, and `j`/`k` type into the filter.
- **UI:** Tab bar, pane frames, status bar, help overlay, rename prompt, search bar, and default/dark/light chrome themes; the `Ctrl-b` prefix key map, with `?` for help; terminal cells keep their program colors.
- **Settings:** Startup config at `$XDG_CONFIG_HOME/herdr-terminal/config.json` (HOME fallback) for theme, scrollback, mouse, clipboard and prefix keys; bad values warn and fall back. Settings are read once at startup; there is no settings UI or live reload.
- **Performance:** Bounded per-frame work and bounded scrollback/search keep input responsive during large output.
- **CLI:** `ht` opens the workspace, `ht screen` runs or reattaches to one terminal, `ht probe` copies raw attach bytes; `--version`, `--help`, `--hc`, `--cols`, `--rows`, `-v`.
- **Distribution:** macOS Apple Silicon package with `ht`, license and generated third-party notices; GitHub Actions CI and a tag-triggered GitHub release workflow. The package requires Herdr Core 0.1.17 or newer and does not include a Core.

**Planned**

- **UI:** General confirm/toast prompts.
- **Workspace polish:** Tab reorder, custom layouts, accessibility.
- **Files:** Directory listing and a simple file picker.
- **Distribution:** Install script, `--skill`, CLI docs, `ht status` and core/log diagnostics.
- **Mods/plugins (postponed):** Later via the manifest + external-command model, not JS/GPUI.
- **Kitty images (postponed):** Later if there is demand.

**Not supported**

- Embedded code editor (use `$EDITOR`), Markdown/rich text, images without a graphics protocol, animations, OS windows, pixel styling, font selection, GPUI/JS mods, screen-based agent detection, Herdr App's private document/labels, multi-window, telemetry/update UI.

See the [architecture](docs/02-architecture.md) for the feature scope, the implemented set, and the reasons behind planned and unsupported features.

## Design constraints

- **Core runs the programs; the client draws the screen.** Herdr Core owns PTYs, terminal lifetime, raw output, events and status; HerdR Terminal owns spaces, tabs, panes, layout and input.
- **Status is only OSC 7501.** The client shows what programs report through the Program Status Protocol; it never scrapes the screen or guesses from process names.
- **One long-lived connection.** A dropped connection does not mean the programs died: the client reconnects and reattaches.
- **Own state.** The workspace lives in the client's `herdr-terminal` document; Herdr App's document and `herdr.space`/`herdr.tab` labels are never touched.
- **Local UI state.** Scrollback, search, selection and theme stay in the client and never enter the shared document or per-screen view; settings are read from a local file once at startup, with no live reload.

## Documentation

- [Core and App Reference](docs/01-core-and-app-reference.md) — what Core and App own, how they talk, and what the TUI can reuse; includes a [glossary](docs/01-core-and-app-reference.md#9-glossary) of Core, attach, VT, credit, document, label and status terms.
- [Architecture](docs/02-architecture.md) — HerdR Terminal's design, feature scope, implemented set, unsupported features, and planned work.
- [AGENTS.md](AGENTS.md) — project rules, traps, the current source layout, and the intended workflow for agents.

## Names

- Product: **HerdR Terminal**
- Repository, package, core document: `herdr-terminal`
- Binary: `ht`
- License: [Apache-2.0](LICENSE)
