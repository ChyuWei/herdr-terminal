# Changelog

One user-visible line per change. Release notes are written at release time
under `## [<version>] - YYYY-MM-DD`; normal work does not edit this file. The
`Unreleased` section holds notes for the next release until it is tagged.

## [Unreleased]

- Requires Herdr Core 0.1.17 or newer; older or unreadable cores are refused at connect with an update hint.
- Adds a macOS Apple Silicon package with the binary, license and generated third-party notices.
- Adds GitHub Actions CI and a tag-triggered macOS release build.
- Adds a Core-wide terminal manager with Show, Hide and a read-only Preview.
- Adds a machine manager for Core machines and remote terminals, with rename recovery.
