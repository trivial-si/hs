# Making a release

The maintainer publishes releases from reviewed commits on `main` after the required checks pass.

1. Review version changes in `Cargo.toml` and `Cargo.lock`, the changelog, migration notes, and third-party notices.
2. Run formatting, clippy, tests, release build, and `cargo package --locked`. Check the minimum supported Rust version in CI.
3. Smoke-test installation, a separate trial database, backup, and import fixtures. Never use personal history in release artifacts.
4. Push a tag matching the package version, such as `v0.1.0`.
5. The release workflow reruns checks, builds native archives for `linux-x86_64` (Ubuntu 22.04), `linux-aarch64` (Ubuntu 24.04 ARM), `macos-aarch64` (Apple Silicon), and `macos-x86_64` (Intel cross-compiled on macOS), extracts release notes from the matching section of `CHANGELOG.md` plus `.github/release-notes.md`, and creates a **draft** GitHub release containing all four archives and a combined `SHA256SUMS` file. Each archive includes the executable, shell completions, license, and notices; GitHub provides matching source archives.
6. Inspect the draft artifacts and release notes, then publish the draft. After publishing, update checksums in the packaging templates with `python3 scripts/update-packaging-checksums.py TAG SHA256SUMS`.

The Cargo package is named `hs-time` (installing the `hs` binary) so it can be published to crates.io (`cargo publish --locked`) without conflicting with the existing `hs` crate. Packaging templates live in [packaging/homebrew/README.md](../packaging/homebrew/README.md) and [packaging/aur/README.md](../packaging/aur/README.md). Never claim a published crate, Homebrew tap, AUR package, or supported platform before verifying it.
