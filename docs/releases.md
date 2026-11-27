# Making a release

The maintainer publishes releases from reviewed commits on `main` after the required checks pass.

1. Review version changes in `Cargo.toml` and `Cargo.lock`, the changelog, migration notes, and third-party notices.
2. Run formatting, clippy, tests, release build, and `cargo package --locked`. Check the minimum supported Rust version in CI.
3. Smoke-test installation, a separate trial database, backup, and import fixtures. Never use personal history in release artifacts.
4. Push a tag matching the package version, such as `v0.1.0`.
5. The release workflow reruns checks, builds on Ubuntu 22.04, extracts release notes from the matching section of `CHANGELOG.md` plus `.github/release-notes.md`, and creates a **draft** GitHub release containing the Linux x86-64 archive and SHA-256 checksums. The archive includes the executable, shell completions, license, and notices; GitHub provides matching source archives.
6. Inspect the draft artifacts and release notes, then publish the draft. Document that the binary targets Linux x86-64 with glibc 2.35 or newer; other Linux setups can build from source.

The Cargo package is named `hs-time` (installing the `hs` binary) so it can be published to crates.io (`cargo publish --locked`) without conflicting with the existing `hs` crate. Arch Linux AUR packaging templates live in [packaging/aur/README.md](../packaging/aur/README.md). Never claim a published crate, AUR package, or supported platform before verifying it.
