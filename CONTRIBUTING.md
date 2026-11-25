# Contributing to hs

Thanks for helping. Documentation fixes, reproducible bug reports, testing on Linux distributions, accessibility feedback, and focused code changes all count.

## Start small

Check existing issues first. Small documentation fixes can go straight to a pull request. For a substantial feature, database change, or incompatible behavior change, open an issue to agree on scope before implementation. A `good first issue` should identify the relevant files and a concrete way to check the result.

## Development setup

Fork the repository, clone your fork, and create a topic branch. Install the pinned toolchain in `rust-toolchain.toml`, including rustfmt and clippy, plus a C compiler. Linux is the initial supported platform. The minimum Rust version is declared in `Cargo.toml` and checked separately in CI.

```sh
cargo build --locked
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo package --locked
```

The integration tests use temporary databases. For manual testing, always pass a disposable database, for example `cargo run -- --db ./trial.db tui`; do not test changes against your daily history. Never include personal databases or client task names in an issue or fixture.

The [architecture guide](docs/architecture.md) maps the code. CLI and TUI live in `src/cli`, workflows in `src/application`, rules in `src/domain`, and SQLite code in `src/storage` and `migrations`.

## Pull requests

Explain the user-visible problem, the change, and how you verified it. Keep unrelated changes separate. Bug fixes should include a focused regression test when practical. Database changes need migration and recovery coverage; timer changes need explicit clock/timezone cases. Update the relevant documentation and changelog. Regenerate shell completions if command options change.

AI-assisted contributions are welcome. You remain responsible for understanding the change, checking its provenance, and running the relevant checks. Describe material assistance when it helps a reviewer understand how the change was produced. Do not submit unverified generated patches or fabricated test results.

Contributions use the project's GPL-3.0-or-later license. Preserve notices for adapted code, tests, and assets, and identify the upstream path and revision. No separate contributor agreement is required.

## Review and maintenance

Ivan Filković (`@ifilkovic`) is the initial maintainer. Contributors retain credit for their work; maintainers review scope, correctness, data compatibility, and documentation before merging. Reviews are best effort, with no guaranteed response time. Use a follow-up on the existing PR rather than opening duplicates.

Release procedure is in [docs/releases.md](docs/releases.md). Project direction is in [ROADMAP.md](ROADMAP.md). Community expectations and the private reporting contact are in [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
