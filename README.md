[English](README.md) | [简体中文](README.zh.md)

# HerdR Terminal

The terminal client for **Herdr Core**.

HerdR Terminal is a terminal UI (TUI) client of **Herdr Core**, the runtime that runs the programs and keeps their terminals alive. It is a sibling of the Herdr App desktop client and a replacement for the old Herdr 1.x TUI. HerdR Terminal owns the screen and workspace organization; Herdr Core owns the programs and the facts about them.

**Status:** implemented and unreleased. The client connects to Herdr Core, renders terminals through a local VT, runs a spaces/tabs/panes workspace with restore and agent awareness, and keeps programs running across detach and reconnect. Later features remain planned.

## Features

**Implemented**

- **Connection:** Core discovery, one long-lived framed connection, `hello` check, reconnect without losing running programs.
- **Terminals:** Run, attach, local VT rendering, keyboard and mouse input, resize, focus, display facts, credit-based flow control, and the `lost`/`restarted`/`finished` lifecycle.
- **Workspace:** Create spaces, tabs and panes; split/close/move; focus, zoom; rename tabs and spaces.
- **Terminal management:** Labels, keep, kill/rm, shell or command.
- **Agents:** Status, turn, title, foreground, agent sidebar, peek, jump, seen/pinned, blocked/finished alerts.
- **State:** Own `herdr-terminal` document, restore, multi-screen sync; rediscover terminals by labels.
- **Recovery:** Detach keeps programs running; reconnect, reattach, and resnapshot after a missed event.
- **UI:** Tab bar, pane frames, status bar, help overlay, rename prompt; the `Ctrl-b` prefix key map, with `?` for help.
- **CLI:** `ht` opens the workspace, `ht screen` runs or reattaches to one terminal, `ht probe` copies raw attach bytes; `--version`, `--help`, `--hc`, `--cols`, `--rows`, `-v`.

**Planned**

- **UI:** Command palette, configurable keybindings, settings, themes; general confirm/toast prompts.
- **Workspace polish:** Mouse resize/drag, tab reorder, scrollback search, copy/paste, custom layouts.
- **Machines:** Add/connect/list/rename/rm, remote terminals and events.
- **Files:** Read/write/edit, diff, external editor, simple file picker.
- **Distribution:** Packaging, install, `--skill`, CLI docs, `ht status` and core/log diagnostics.
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

## Documentation

- [Core and App Reference](docs/01-core-and-app-reference.md) — what Core and App own, how they talk, and what the TUI can reuse; includes a [glossary](docs/01-core-and-app-reference.md#9-glossary) of Core, attach, VT, credit, document, label and status terms.
- [Architecture](docs/02-architecture.md) — HerdR Terminal's design, feature scope, implemented set, unsupported features, and phases.
- [AGENTS.md](AGENTS.md) — project rules, traps, the current source layout, and the intended workflow for agents.

## Names

- Product: **HerdR Terminal**
- Repository, package, core document: `herdr-terminal`
- Binary: `ht`
- License: [Apache-2.0](LICENSE)
