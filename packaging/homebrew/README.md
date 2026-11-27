# Homebrew tap packaging

[hs.rb](hs.rb) is a Homebrew formula template for macOS (Apple Silicon and Intel) and Linux (x86-64 and ARM64). It installs `hs` and shell completions for Bash, Zsh, and Fish from the official GitHub Release archives.

## Publish or update the tap

1. Create a public GitHub repository named `trivial-si/homebrew-tap` (once).
2. After publishing a GitHub release (such as `v0.1.2`), download its `SHA256SUMS` file and populate checksums automatically:

```sh
python3 scripts/update-packaging-checksums.py v0.1.2 /path/to/SHA256SUMS
```

3. Copy `packaging/homebrew/hs.rb` into `Formula/hs.rb` in `trivial-si/homebrew-tap`, commit, and push:

```sh
git clone git@github.com:trivial-si/homebrew-tap.git /tmp/homebrew-tap
mkdir -p /tmp/homebrew-tap/Formula
cp packaging/homebrew/hs.rb /tmp/homebrew-tap/Formula/hs.rb
cd /tmp/homebrew-tap
git add Formula/hs.rb
git commit -m "hs 0.1.2"
git push
```

Users can then install and upgrade with:

```sh
brew install trivial-si/tap/hs
```
