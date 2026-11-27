## Archives and installation

Each archive contains `hs`, `LICENSE`, `NOTICE`, and `completions/`:

- `linux-x86_64` (glibc 2.35 or newer)
- `linux-aarch64` (glibc 2.39 or newer)
- `macos-aarch64` (Apple Silicon)
- `macos-x86_64` (Intel)

Matching source archives are attached below; other environments can build from source.

Verify the downloaded archive against `SHA256SUMS`, extract it, and install `hs` on your `PATH`. On macOS, binaries downloaded through a browser may require clearing quarantine (`xattr -d com.apple.quarantine ./hs`) because release binaries are currently unsigned.

See `README.md` for installation steps, `docs/guide.md` for shell completion setup, and `docs/hamster.md` before importing a Project Hamster database. Keep backups of important history.

Contributions, bug reports, and platform testing feedback are welcome.
