# Local equivalents of CI. `just check` is what ci.yml runs; `just package`
# builds the macOS archive that a tag publishes.
set shell := ["sh", "-cu"]
set windows-shell := ["powershell.exe", "-NoLogo", "-NoProfile", "-Command"]

# Build the debug binary.
build:
    cargo build --locked

# Run the whole suite.
test:
    cargo test --locked

# Formatting, clippy and the suite: the CI checks.
check: fmt-check lint
    cargo test --locked

fmt:
    cargo fmt

fmt-check:
    cargo fmt --check

lint:
    cargo clippy --locked --all-targets -- -D warnings

# Regenerate THIRD-PARTY-NOTICES after a dependency change.
[unix]
notices:
    scripts/notices

# Build dist/herdr-terminal-<version>-macos-<arch>.tar.gz.
[unix]
package:
    scripts/package

# Bump <version>, require its CHANGELOG.md heading and run checks. Commits
# nothing; review the bump, then publish.
[unix]
release-prepare version:
    scripts/release-prepare "{{version}}"

# Push the current branch, wait for CI, then tag and push v<version>.
[unix]
release-publish version:
    scripts/release-publish "{{version}}"
