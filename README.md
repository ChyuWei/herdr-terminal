[English](README.md) | [简体中文](README.zh.md)

# HerdR Terminal

The terminal client for **Herdr Core**.

HerdR Terminal is a TUI client of Herdr Core, a sibling of Herdr App and a replacement for the old Herdr 1.x TUI. It owns the terminal screen and workspace organization; Herdr Core owns the programs and the facts about them.

**Status:** design phase. Cargo workspace initialized; the TUI is not implemented yet.

## Features

**v1 — first usable release**

- **Connection:** Core discovery, one long-lived framed connection, `hello` check, reconnect without losing running programs.
- **Terminals:** Run, attach, local VT rendering, keyboard and mouse input, resize, focus, display facts.
- **Workspace:** Spaces, tabs, panes, split/close/move, focus, zoom.
- **Terminal management:** New/close/rename, labels, keep, kill/rm, shell or command.
- **Agents:** Status, turn, title, foreground, agent list/sidebar, peek, seen/pinned.
- **UI:** Tab bar, pane frames, status bar, command palette, help, toast/confirm/prompt, themes.
- **State:** Own `herdr-terminal` document, restore, multi-screen sync; rediscover terminals by labels.
- **Recovery:** `lost`, `restarted`, `finished`, `missed`, heartbeat, reattach.
- **Diagnostics:** Version, core status, logs, `ht status`.

**Later**

- **Workspace polish:** Mouse resize/drag, scrollback search, copy/paste, custom layouts.
- **Machines:** Add/connect/list/rename/rm, remote terminals and events.
- **Files:** Read/write/edit, diff, external editor, simple file picker.
- **Distribution:** Packaging, install, `--skill`, CLI docs.
- **Mods/plugins (postponed):** Later via the manifest + external-command model, not JS/GPUI.
- **Kitty images (postponed):** Later if there is demand.

**Not supported**

- Embedded code editor (use `$EDITOR`), Markdown/rich text, images without a graphics protocol, animations, OS windows, pixel styling, font selection, GPUI/JS mods, screen-based agent detection, Herdr App's private document/labels, multi-window, telemetry/update UI.

See the [architecture](docs/02-architecture.md) for the full status table and reasons.

## Documentation

- [Core and App Reference](docs/01-core-and-app-reference.md) — what Core and App own, how they talk, and what the TUI can reuse.
- [Architecture](docs/02-architecture.md) — HerdR Terminal's design, feature scope, unsupported features, and phases.
- [AGENTS.md](AGENTS.md) — project rules, traps, and the intended workflow for agents.

## Names

- Product: **HerdR Terminal**
- Repository, package, core document: `herdr-terminal`
- Binary: `ht`
- License: [Apache-2.0](LICENSE)
