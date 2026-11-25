# Changelog

## Unreleased

- Updated clap to 4.6.9, rustix to 1.1.6, sha2 to 0.11.1, signal-hook to 0.4.6, and unicode-width to 0.2.3. Request and import fingerprints are unchanged.
- Updated the pinned `actions/checkout` CI action to 7.0.1.
- Documented a verified installation on Arch Linux in the user guide.
- Added a reproducible terminal demo with fictional tasks, a temporary database, and a screenshot of the interface.

## 0.1.0 (2026-11-25)

Initial public release of hs by Trivial.si.

- Local SQLite time tracking, categories, tags, reports, and exports.
- Tasks, an Eisenhower matrix, and a keyboard-driven terminal interface.
- Pomodoro sessions with persisted timers and optional completion sounds.
- Import of Project Hamster schema-v9 history into a separate database.
- JSON API and stdio MCP integration with previews, revision checks, and retry receipts.
- Backup support, schema migrations, and shell completion.
- Contributor documentation, Hamster credits, and the existing GPL-3.0-or-later license.

Linux is the initial supported platform. This early release does not promise complete Project Hamster feature parity. See the migration guide before importing important history.
