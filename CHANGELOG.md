# Changelog

## Unreleased

- Documented a source installation and disposable SQLite smoke test on Debian 12.

## 0.1.2 (2026-11-27)

- Added multi-platform prebuilt release archives in `.github/workflows/release.yml` for `macos-aarch64` (Apple Silicon), `macos-x86_64` (Intel), `linux-aarch64` (ARM64), and `linux-x86_64`, each bundled with `completions/`, `LICENSE`, and `NOTICE` plus a combined `SHA256SUMS`.
- Added a Homebrew formula template (`packaging/homebrew/hs.rb`) covering macOS and Linux, plus `scripts/update-packaging-checksums.py` to populate Homebrew and AUR SHA-256 checksums from a release `SHA256SUMS` file.
- Updated installation and release documentation for macOS and Linux ARM64 archives.

## 0.1.1 (2026-11-27)

- Renamed the Cargo package to `hs-time` (keeping the `hs` binary and `hs` library name) so it is ready for crates.io publication, relaxed exact dependency pins to semver ranges while preserving `--locked` builds, and grouped Dependabot updates.
- Bundled `completions/` in Linux release archives, assembled draft release notes from `CHANGELOG.md` via `scripts/extract-release-notes.py`, read the pinned CI toolchain from `rust-toolchain.toml`, and added macOS, rustdoc, and terminal demo checks to CI.
- Added Arch Linux AUR packaging templates (`packaging/aur/hs-time-bin` and `packaging/aur/hs-time`) and documented a verified Arch Linux installation in the user guide.
- Added a reproducible terminal demo (`docs/demo.sh`) with fictional tasks, a static screenshot, and an animated GIF recorder (`scripts/record-demo-gif.py`).
- Documented prebuilt release installation, `cargo install --git`, persistent shell completion setup, client-neutral MCP task classification, and pre-1.0 library stability.
- Updated branding across docs and social artwork to lowercase `trivial.si`.
- Made `tests/tui_process.rs` reconstruct rendered screen cells so `cargo test` passes reliably inside 80x24 interactive terminals.
- Updated clap to 4.6.9, rustix to 1.1.6, sha2 to 0.11.1, signal-hook to 0.4.6, and unicode-width to 0.2.3. Request and import fingerprints are unchanged.
- Updated the pinned `actions/checkout` CI action to 7.0.1.

## 0.1.0 (2026-11-25)

Initial public release of hs by trivial.si.

- Local SQLite time tracking, categories, tags, reports, and exports.
- Tasks, an Eisenhower matrix, and a keyboard-driven terminal interface.
- Pomodoro sessions with persisted timers and optional completion sounds.
- Import of Project Hamster schema-v9 history into a separate database.
- JSON API and stdio MCP integration with previews, revision checks, and retry receipts.
- Backup support, schema migrations, and shell completion.
- Contributor documentation, Hamster credits, and the existing GPL-3.0-or-later license.

Linux is the initial supported platform. This early release does not promise complete Project Hamster feature parity. See the migration guide before importing important history.
