# Architecture

[Back to hs](../README.md)

## Architecture and development

One Cargo package contains the `hs` binary and a Rust library. A command enters through the CLI, runs an application workflow or query, and uses the same domain rules as library callers.

| Source | Responsibility |
| --- | --- |
| `src/wire.rs` | Decode bounded JSON and reject duplicate keys, including escaped spellings of the same key. Both agent transports use this decoder. |
| `src/cli/` | Parse terminal arguments and render command output. `mcp.rs` handles stdio framing and session lifecycle, `mcp/initialize.rs` validates the handshake, and `mcp/tools.rs` maps calls to the agent API. |
| `src/cli/tui/` | Keep the keyboard event loop, Ratatui rendering, and terminal lifecycle separate. Audio runs outside the rendering thread; terminal cleanup covers normal exit, errors, panics, and supported termination signals. |
| `src/application/agent/` | Validate the published schema, then decode the input once into a typed Rust `Command` in `command.rs`. Workflows receive checked IDs, activity details and edit fields. The agent layer checks preconditions, builds previews and records receipts. |
| `src/application/` | Coordinate tracking, tasks, Pomodoro sessions and imports. `workflows` owns commands that touch multiple features, `editing` resolves edits against the stored entry, `queries` builds consistent read views, and `clock` reads the system time and timezone. |
| `src/domain/` | Define task and timer states, parse quick entries, calculate workday boundaries and report durations. `time::Moment` pairs a UTC instant with an IANA zone. `workday::Workday` handles civil boundaries, including repeated or skipped clock times. |
| `src/storage/` and `migrations/` | Implement SQLite queries and writes, database initialization, migration and backup. Workflows pass their transaction to concrete storage functions when changing related rows. |

For example, completing a task checks which timer it owns, closes any associated work and records completion in one transaction. A failure rolls back the whole change. Manual tracking and Pomodoro both use the same entry operations. A manual retry can reuse a matching running entry, while each new focus segment receives its own ID.

`application::interactive` checks the displayed timer identity and phase under the same transaction lock as a TUI mutation. The terminal UI retains selection by task ID, guards edits against concurrent changes to the edited field, and uses a short connection lock timeout so navigation and quitting remain responsive. It refreshes once per second and after mutations; the JSON and MCP paths keep their existing contracts.

Human CLI status, history and task views read one database snapshot. If a reader sees a running Pomodoro, it releases the read transaction, takes the writer lock, checks the session again, and materializes the requested view after applying any elapsed deadline. An idle or paused view uses a read transaction. Agent context and query operations use a separate read-only path and never reconcile timers. SQLite can create coordination sidecars when reading an externally configured WAL database; logical data is unchanged. Reports calculate elapsed work using the snapshot’s cutoff and choose calendar periods using the invocation date.

The command API distinguishes `domain::ids::EntryId` and `TaskId`. `domain::model::ActivityTemplate` holds reusable activity details and its `starting_at` method creates a `NewEntry`. The domain restores a Pomodoro session into a validated state. Its `state()`, `phase()`, `entry_id()`, `deadline()` and `remaining(&moment)` methods expose the timer details. A running focus owns a work segment, paused work retains the remaining duration, and a ready session offers the next phase.

Existing library paths such as `hs::tracking`, `hs::snapshot`, `hs::pomodoro`, `hs::model` and `hs::store` re-export the corresponding modules. The `hs::time` compatibility module keeps `Moment::system()` and `Moment::from_local(...)`. New code can call `application::clock::now()` at the command boundary and pass a `Moment` to the workflow. Tests use the same path with an explicit instant and timezone.

Run the development checks from the package directory.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo package --locked
```

The local audit used the checkout’s pinned Rust 1.100.0 toolchain. The declared minimum is Rust 1.88. The CI workflow includes a separate `cargo +1.88.0 check --locked --all-targets` job.

The license is GPL-3.0-or-later. See [LICENSE](../LICENSE) and [upstream credits](../NOTICE).
